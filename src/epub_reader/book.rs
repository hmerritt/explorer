use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use quick_xml::{
    Reader, Writer,
    events::{BytesStart, Event},
};
use rbook::{Epub, epub::toc::EpubTocEntry};

const COVER_HREF: &str = "explorer:epub-cover";

#[derive(Clone, Debug)]
pub(super) struct Section {
    pub href: String,
    pub title: String,
}

#[derive(Clone, Debug)]
pub(super) struct TocItem {
    pub title: String,
    pub target: Option<String>,
    pub depth: usize,
}

pub(super) struct Book {
    pub epub: Arc<Epub>,
    pub title: String,
    pub sections: Vec<Section>,
    pub toc: Vec<TocItem>,
    pub cover_href: Option<String>,
    cover_chapter: Option<Chapter>,
    cover_aliases: Vec<String>,
}

impl Book {
    pub fn open(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        check_cancel(cancel)?;
        let epub = Epub::open(path).map_err(|e| format!("Could not open EPUB: {e}"))?;
        if epub
            .metadata()
            .by_property("rendition:layout")
            .any(|meta| meta.value() == "pre-paginated")
            || epub.spine().iter().any(|entry| {
                entry
                    .properties()
                    .iter()
                    .any(|p| p == "rendition:layout-pre-paginated")
            })
        {
            return Err(
                "Fixed-layout EPUBs are not supported. Open this book in another reader.".into(),
            );
        }
        // Font obfuscation does not prevent reading: publisher fonts are ignored.
        if epub.contains_resource("/META-INF/encryption.xml") {
            let encryption = epub
                .read_resource_str("/META-INF/encryption.xml")
                .map_err(|e| format!("Could not read EPUB encryption information: {e}"))?;
            check_encryption(&encryption)?;
        }
        let title = epub
            .metadata()
            .title()
            .map(|title| title.value().to_owned())
            .unwrap_or_else(|| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let mut toc = Vec::new();
        if let Some(contents) = epub.toc().contents() {
            for entry in contents.iter() {
                collect_toc(entry, 0, &mut toc);
            }
        }
        for item in &mut toc {
            let usable = item.target.as_deref().is_some_and(|target| {
                epub.manifest()
                    .by_href(target.split('#').next().unwrap_or(target))
                    .is_some_and(|resource| {
                        matches!(
                            resource.media_type(),
                            "application/xhtml+xml" | "text/html" | "image/svg+xml"
                        )
                    })
            });
            if !usable {
                item.target = None;
            }
        }
        let mut sections = Vec::new();
        for entry in epub.spine().iter().filter(|entry| entry.is_linear()) {
            let resource = entry
                .manifest_entry()
                .ok_or("EPUB reading order references a missing resource.")?;
            let href = resource.href().path().as_str().to_owned();
            let title = toc
                .iter()
                .find(|item| {
                    item.target
                        .as_ref()
                        .is_some_and(|target| target.split('#').next() == Some(href.as_str()))
                })
                .map(|item| item.title.clone())
                .unwrap_or_else(|| format!("Chapter {}", sections.len() + 1));
            sections.push(Section { href, title });
        }
        if sections.is_empty() {
            return Err("This EPUB has no readable chapters.".into());
        }
        // Only inspect the declared cover and, when needed, the opening
        // document. The rest of the book remains lazily loaded.
        let cover_image = epub
            .manifest()
            .cover_image()
            .map(|entry| entry.href().path().as_str().to_owned());
        let declared_cover = epub.toc().landmarks().and_then(find_cover_document);
        let read_chapter = |href: &str| {
            let resource = epub.manifest().by_href(href)?;
            if !matches!(
                resource.media_type(),
                "application/xhtml+xml" | "text/html" | "image/svg+xml"
            ) {
                return None;
            }
            parse_chapter(&resource.read_str().ok()?, href, cancel)
                .ok()
                .filter(|chapter| !chapter.blocks.is_empty())
        };
        let mut cover = declared_cover
            .as_ref()
            .and_then(|href| read_chapter(href).map(|chapter| (href.clone(), chapter)));
        if cover.is_none() {
            if let Some(href) = &declared_cover {
                sections.retain(|section| section.href != *href);
                toc.retain(|item| {
                    item.target
                        .as_deref()
                        .and_then(|target| target.split('#').next())
                        != Some(href)
                });
            }
        }
        if sections.is_empty() && cover.is_none() {
            return Err("This EPUB has no readable chapters.".into());
        }
        let mut cover_aliases = Vec::new();
        if let Some(image) = &cover_image {
            let first_href = sections
                .first()
                .map(|section| section.href.clone())
                .unwrap_or_default();
            if cover.as_ref().is_none_or(|(href, _)| *href != first_href) {
                if let Some(chapter) = read_chapter(&first_href) {
                    if matches!(chapter.blocks.as_slice(), [Block::Image { source: ImageSource::Resource(href), .. }] if href.split('#').next() == Some(image.as_str()))
                    {
                        if cover.is_none() {
                            cover = Some((first_href, chapter));
                        } else {
                            cover_aliases.push(first_href.clone());
                            sections.remove(0);
                        }
                    }
                }
            }
            if cover.is_none() {
                cover = Some((
                    COVER_HREF.into(),
                    Chapter {
                        blocks: vec![Block::Image {
                            source: ImageSource::Resource(image.clone()),
                            alt: String::new(),
                        }],
                        anchors: HashMap::new(),
                        is_cover: true,
                    },
                ));
            }
        }
        let (cover_href, cover_chapter) = if let Some((href, mut chapter)) = cover {
            chapter.is_cover = true;
            sections.retain(|section| section.href != href);
            sections.insert(
                0,
                Section {
                    href: href.clone(),
                    title: "Cover".into(),
                },
            );
            for item in &mut toc {
                if item.target.as_deref().is_some_and(|target| {
                    target.split('#').next().is_some_and(|target| {
                        target == href || cover_aliases.iter().any(|alias| alias == target)
                    })
                }) {
                    item.target = Some(href.clone());
                }
            }
            toc.retain(|item| item.target.as_deref() != Some(href.as_str()));
            (Some(href), Some(chapter))
        } else {
            (None, None)
        };
        if !toc.iter().any(|item| item.target.is_some()) {
            toc = sections
                .iter()
                .filter(|section| Some(&section.href) != cover_href.as_ref())
                .map(|section| TocItem {
                    title: section.title.clone(),
                    target: Some(section.href.clone()),
                    depth: 0,
                })
                .collect();
        }
        if let Some(href) = &cover_href {
            toc.insert(
                0,
                TocItem {
                    title: "Cover".into(),
                    target: Some(href.clone()),
                    depth: 0,
                },
            );
        }
        check_cancel(cancel)?;
        Ok(Self {
            epub: Arc::new(epub),
            title,
            sections,
            toc,
            cover_href,
            cover_chapter,
            cover_aliases,
        })
    }

    pub fn chapter(&self, href: &str, cancel: &AtomicBool) -> Result<Chapter, String> {
        check_cancel(cancel)?;
        if self.is_cover(href) {
            return Ok(self.cover_chapter.clone().unwrap_or_default());
        }
        let resource = self
            .epub
            .manifest()
            .by_href(href)
            .ok_or("EPUB chapter is missing from the manifest.")?;
        let text = resource
            .read_str()
            .map_err(|e| format!("Could not read chapter: {e}"))?;
        parse_chapter(&text, href, cancel)
    }

    pub fn is_cover(&self, href: &str) -> bool {
        self.cover_href.as_deref() == Some(href)
            || self.cover_aliases.iter().any(|alias| alias == href)
    }

    pub fn reading_href<'a>(&'a self, href: &'a str) -> &'a str {
        if self.is_cover(href) {
            self.cover_href.as_deref().unwrap_or(href)
        } else {
            href
        }
    }

    pub fn readable_location(&self, href: &str) -> bool {
        self.is_cover(href)
            || self.epub.manifest().by_href(href).is_some_and(|entry| {
                matches!(
                    entry.media_type(),
                    "application/xhtml+xml" | "text/html" | "image/svg+xml"
                )
            })
    }

    pub fn image_data(&self, href: &str) -> Result<(Vec<u8>, bool), String> {
        let href = href.split('#').next().unwrap_or(href);
        let resource = self
            .epub
            .manifest()
            .by_href(href)
            .ok_or("Image is missing from the EPUB.")?;
        let svg = resource.media_type().eq_ignore_ascii_case("image/svg+xml")
            || href.to_ascii_lowercase().ends_with(".svg");
        let bytes = resource
            .read_bytes()
            .map_err(|e| format!("Could not read image: {e}"))?;
        Ok((bytes, svg))
    }
}

fn find_cover_document(entry: EpubTocEntry<'_>) -> Option<String> {
    if entry
        .kind_raw()
        .is_some_and(|kind| kind.split_whitespace().any(|kind| kind == "cover"))
    {
        return entry.href().map(|href| href.path().as_str().to_owned());
    }
    entry.iter().find_map(find_cover_document)
}

fn collect_toc(entry: EpubTocEntry<'_>, depth: usize, items: &mut Vec<TocItem>) {
    items.push(TocItem {
        title: entry.label().to_owned(),
        target: entry.href().map(|href| href.as_str().to_owned()),
        depth,
    });
    for child in entry.iter() {
        collect_toc(child, depth + 1, items);
    }
}

fn check_encryption(xml: &str) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader
            .read_event()
            .map_err(|e| format!("Invalid encryption information: {e}"))?
        {
            Event::Start(tag) | Event::Empty(tag)
                if tag.local_name().as_ref() == "EncryptionMethod" =>
            {
                let algorithm = attr(&tag, b"Algorithm");
                if !matches!(
                    algorithm.as_deref(),
                    Some("http://www.idpf.org/2008/embedding" | "http://ns.adobe.com/pdf/enc#RC")
                ) {
                    return Err("DRM-protected EPUBs are not supported. Open this book in an authorized reader.".into());
                }
            }
            Event::Start(tag) | Event::Empty(tag)
                if tag.local_name().as_ref() == "CipherReference" =>
            {
                if attr(&tag, b"URI").is_some_and(|uri| {
                    ![".ttf", ".otf", ".woff", ".woff2"]
                        .iter()
                        .any(|extension| uri.to_ascii_lowercase().ends_with(extension))
                }) {
                    return Err("Encrypted EPUB reading content is not supported. Open this book in an authorized reader.".into());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Reading cancelled.".into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Span {
    pub range: std::ops::Range<usize>,
    pub style: InlineStyle,
}

#[derive(Clone, Debug)]
pub(super) enum ImageSource {
    Resource(String),
    Svg { bytes: Vec<u8>, base_href: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TextKind {
    pub heading: u8,
    pub indent: usize,
    pub quote: bool,
    pub pre: bool,
}

#[derive(Clone, Debug)]
pub(super) enum Block {
    Text {
        text: String,
        spans: Vec<Span>,
        kind: TextKind,
    },
    Image {
        source: ImageSource,
        alt: String,
    },
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Deserialize,
    serde::Serialize,
)]
pub(super) struct ContentPoint {
    pub block: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Chapter {
    pub blocks: Vec<Block>,
    pub anchors: HashMap<String, ContentPoint>,
    pub is_cover: bool,
}

#[derive(Default)]
struct TextBuilder {
    text: String,
    spans: Vec<Span>,
    space: bool,
}

impl TextBuilder {
    fn append(&mut self, text: &str, style: &InlineStyle, pre: bool) {
        for ch in text.chars() {
            if !pre && ch.is_whitespace() && ch != '\u{a0}' {
                self.space = !self.text.is_empty();
                continue;
            }
            if self.space {
                self.push(' ', style);
                self.space = false;
            }
            self.push(ch, style);
        }
    }
    fn push(&mut self, ch: char, style: &InlineStyle) {
        let start = self.text.len();
        self.text.push(ch);
        if let Some(span) = self.spans.last_mut().filter(|span| span.style == *style) {
            span.range.end = self.text.len();
        } else {
            self.spans.push(Span {
                range: start..self.text.len(),
                style: style.clone(),
            });
        }
    }
    fn flush(&mut self, chapter: &mut Chapter, kind: &TextKind) {
        if !self.text.is_empty() {
            chapter.blocks.push(Block::Text {
                text: std::mem::take(&mut self.text),
                spans: std::mem::take(&mut self.spans),
                kind: kind.clone(),
            });
        }
        self.space = false;
    }
}

fn attr(tag: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    tag.attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref().as_bytes() == key)
        .and_then(|attribute| {
            attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
}

/// Resolve relative book URLs without ever converting them to filesystem paths.
pub(super) fn resolve_href(base: &str, reference: &str) -> Option<String> {
    let base = reqwest::Url::parse(&format!(
        "https://epub.invalid{}",
        if base.starts_with('/') {
            base.to_owned()
        } else {
            format!("/{base}")
        }
    ))
    .ok()?;
    let target = base.join(reference).ok()?;
    if target.host_str() != Some("epub.invalid") || target.scheme() != "https" {
        return None;
    }
    Some(format!(
        "{}{}",
        target.path(),
        target
            .fragment()
            .map_or(String::new(), |fragment| format!("#{fragment}"))
    ))
}

pub(super) fn parse_chapter(xml: &str, href: &str, cancel: &AtomicBool) -> Result<Chapter, String> {
    let mut reader = Reader::from_str(xml);
    let mut chapter = Chapter::default();
    let mut text = TextBuilder::default();
    let mut style = InlineStyle::default();
    let mut kind = TextKind::default();
    let mut stack = Vec::new();
    let mut lists: Vec<Option<usize>> = Vec::new();
    let mut ignored = 0;
    loop {
        check_cancel(cancel)?;
        let event = reader
            .read_event()
            .map_err(|e| format!("Invalid EPUB chapter: {e}"))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let name = tag.local_name().as_ref().to_ascii_lowercase();
                if ignored > 0
                    || matches!(
                        name.as_str(),
                        "head" | "script" | "style" | "audio" | "video"
                    )
                {
                    if !empty {
                        ignored += 1;
                    }
                    continue;
                }
                let block = matches!(
                    name.as_str(),
                    "p" | "div"
                        | "section"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "li"
                        | "blockquote"
                        | "pre"
                        | "tr"
                        | "figure"
                        | "figcaption"
                );
                if block {
                    text.flush(&mut chapter, &kind);
                }
                let old_style = style.clone();
                let old_kind = kind.clone();
                if matches!(name.as_str(), "img" | "image" | "svg") {
                    text.flush(&mut chapter, &kind);
                }
                match name.as_str() {
                    "b" | "strong" => style.bold = true,
                    "i" | "em" | "cite" => style.italic = true,
                    "u" => style.underline = true,
                    "a" => {
                        style.link = attr(&tag, b"href")
                            .map(|reference| resolve_href(href, &reference).unwrap_or(reference))
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        kind.heading = name.as_bytes()[1] - b'0'
                    }
                    "blockquote" => {
                        kind.quote = true;
                        kind.indent += 1;
                    }
                    "pre" => kind.pre = true,
                    "ul" => lists.push(None),
                    "ol" => lists.push(Some(
                        attr(&tag, b"start")
                            .and_then(|start| start.parse().ok())
                            .unwrap_or(1),
                    )),
                    "li" => {
                        kind.indent += lists.len();
                        let prefix = match lists.last_mut() {
                            Some(Some(n)) => {
                                let value = format!("{}. ", *n);
                                *n += 1;
                                value
                            }
                            _ => "• ".into(),
                        };
                        text.append(&prefix, &style, true);
                    }
                    "td" | "th" => {
                        if !text.text.is_empty() {
                            text.append(" | ", &style, true);
                        }
                        if name == "th" {
                            style.bold = true;
                        }
                    }
                    "br" => {
                        text.space = false;
                        text.push('\n', &style);
                    }
                    _ => {}
                }
                if let Some(id) =
                    attr(&tag, b"id").or_else(|| attr(&tag, b"name").filter(|_| name == "a"))
                {
                    chapter.anchors.insert(
                        id,
                        ContentPoint {
                            block: chapter.blocks.len(),
                            offset: text.text.len(),
                        },
                    );
                }
                if matches!(name.as_str(), "img" | "image") {
                    text.flush(&mut chapter, &kind);
                    if let Some(src) = attr(&tag, b"src")
                        .or_else(|| attr(&tag, b"href"))
                        .and_then(|src| resolve_href(href, &src))
                    {
                        chapter.blocks.push(Block::Image {
                            source: ImageSource::Resource(src),
                            alt: attr(&tag, b"alt").unwrap_or_default(),
                        });
                    }
                }
                if name == "svg" && !empty {
                    text.flush(&mut chapter, &kind);
                    let mut writer = Writer::new(Vec::new());
                    writer
                        .write_event(Event::Start(tag.to_owned()))
                        .map_err(|e| e.to_string())?;
                    let mut depth = 1;
                    while depth > 0 {
                        check_cancel(cancel)?;
                        let event = reader.read_event().map_err(|e| e.to_string())?;
                        match event {
                            Event::Start(_) => depth += 1,
                            Event::End(_) => depth -= 1,
                            Event::Eof => return Err("Incomplete inline SVG.".into()),
                            _ => {}
                        }
                        writer.write_event(event).map_err(|e| e.to_string())?;
                    }
                    chapter.blocks.push(Block::Image {
                        source: ImageSource::Svg {
                            bytes: writer.into_inner(),
                            base_href: href.to_owned(),
                        },
                        alt: String::new(),
                    });
                    style = old_style;
                    kind = old_kind;
                    continue;
                }
                if !empty {
                    stack.push((name, old_style, old_kind, block));
                } else {
                    style = old_style;
                    kind = old_kind;
                }
            }
            Event::End(tag) => {
                if ignored > 0 {
                    ignored -= 1;
                    continue;
                }
                let name = tag.local_name().as_ref().to_ascii_lowercase();
                if matches!(name.as_str(), "ul" | "ol") {
                    lists.pop();
                }
                if let Some((_, old_style, old_kind, block)) = stack.pop() {
                    if block {
                        text.flush(&mut chapter, &kind);
                    }
                    style = old_style;
                    kind = old_kind;
                }
            }
            Event::Text(value) if ignored == 0 => {
                let value = value.html_content();
                text.append(&value, &style, kind.pre);
            }
            Event::CData(value) if ignored == 0 => {
                let value = value.html_content();
                text.append(&value, &style, kind.pre);
            }
            Event::GeneralRef(value) if ignored == 0 => {
                let reference = format!("&{};", &*value);
                let value = quick_xml::escape::unescape(&reference).map_err(|e| e.to_string())?;
                text.append(&value, &style, kind.pre);
            }
            Event::Eof => {
                if !stack.is_empty() || ignored != 0 {
                    return Err("Incomplete EPUB chapter markup.".into());
                }
                break;
            }
            _ => {}
        }
    }
    text.flush(&mut chapter, &kind);
    Ok(chapter)
}
