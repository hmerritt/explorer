//! Deliberately bounded publisher CSS: simple/descendant selectors and reading
//! layout properties. Resources are resolved inside the archive only.
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(super) struct ElementStyle {
    pub tag: String,
    pub id: String,
    pub classes: Vec<String>,
}

#[derive(Default)]
pub(super) struct Stylesheet {
    rules: Vec<(String, Vec<(String, String, bool)>)>,
}

fn declarations(text: &str) -> Vec<(String, String, bool)> {
    text.split(';')
        .filter_map(|entry| {
            let (name, value) = entry.split_once(':')?;
            let value = value.trim().to_ascii_lowercase();
            let important = value.ends_with("!important");
            Some((
                name.trim().to_ascii_lowercase(),
                value.trim_end_matches("!important").trim().to_owned(),
                important,
            ))
        })
        .collect()
}

impl Stylesheet {
    pub fn append(&mut self, css: &str) {
        let mut clean = String::new();
        let mut remaining = css;
        while let Some((before, after)) = remaining.split_once("/*") {
            clean.push_str(before);
            let Some((_, tail)) = after.split_once("*/") else {
                return;
            };
            remaining = tail;
        }
        clean.push_str(remaining);
        // Skip at-rules, including their nested bodies. We do not accidentally
        // activate print rules or fetch @import URLs.
        let mut tail = clean.as_str();
        while let Some(open) = tail.find('{') {
            // An ignored statement such as @import must not swallow the next rule.
            let selector = tail[..open].rsplit(';').next().unwrap_or_default().trim();
            let mut depth = 1;
            let mut close = None;
            for (offset, ch) in tail[open + 1..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    close = Some(open + 1 + offset);
                    break;
                }
            }
            let Some(close) = close else {
                break;
            };
            if !selector.contains('@') && !tail[open + 1..close].contains('{') {
                for selector in selector.split(',') {
                    self.rules.push((
                        selector.trim().to_owned(),
                        declarations(&tail[open + 1..close]),
                    ));
                }
            }
            tail = &tail[close + 1..];
        }
    }

    pub fn properties(&self, ancestry: &[ElementStyle], inline: &str) -> BTreeMap<String, String> {
        let mut cascade: BTreeMap<String, ((bool, usize, usize), String)> = BTreeMap::new();
        for (order, (selector, properties)) in self.rules.iter().enumerate() {
            if matches_selector(selector, ancestry) {
                let specificity = selector.matches('#').count() * 100
                    + selector.matches('.').count() * 10
                    + selector
                        .split_whitespace()
                        .filter(|part| part.chars().next().is_some_and(char::is_alphabetic))
                        .count();
                for (name, value, important) in properties {
                    let rank = (*important, specificity, order);
                    if cascade.get(name).is_none_or(|(old, _)| rank >= *old) {
                        cascade.insert(name.clone(), (rank, value.clone()));
                    }
                }
            }
        }
        for (name, value, important) in declarations(inline) {
            let rank = (important, 1000, self.rules.len());
            if cascade.get(&name).is_none_or(|(old, _)| rank >= *old) {
                cascade.insert(name, (rank, value));
            }
        }
        cascade
            .into_iter()
            .map(|(name, (_, value))| (name, value))
            .collect()
    }
}

fn matches_simple(selector: &str, element: &ElementStyle) -> bool {
    if selector.contains([':', '[', ']', '+', '~', '>']) {
        return false;
    }
    // Split components without losing whether they are a class or ID.
    let mut start = 0;
    let mut components = Vec::new();
    for (index, ch) in selector.char_indices() {
        if (ch == '.' || ch == '#') && index > start {
            components.push(&selector[start..index]);
            start = index;
        }
    }
    components.push(&selector[start..]);
    components.iter().all(|part| match part.as_bytes().first() {
        Some(b'.') => element.classes.iter().any(|class| class == &part[1..]),
        Some(b'#') => element.id == part[1..],
        _ => *part == "*" || part.eq_ignore_ascii_case(&element.tag),
    })
}

fn matches_selector(selector: &str, ancestry: &[ElementStyle]) -> bool {
    let mut parts = selector.split_whitespace().rev();
    let Some(last) = parts.next() else {
        return false;
    };
    let Some((current, parents)) = ancestry.split_last() else {
        return false;
    };
    if !matches_simple(last, current) {
        return false;
    }
    let mut parents = parents.iter().rev();
    parts.all(|part| {
        parents
            .by_ref()
            .any(|element| matches_simple(part, element))
    })
}

/// Store lengths in thousandths of an em, so style keys remain exact and Eq.
pub(super) fn em_length(value: &str) -> Option<i32> {
    let (number, scale) = if let Some(value) = value.strip_suffix("em") {
        (value, 1000.0)
    } else if let Some(value) = value.strip_suffix("px") {
        (value, 50.0)
    } else if let Some(value) = value.strip_suffix("pt") {
        (value, 1000.0 / 15.0)
    } else if value == "0" {
        (value, 1000.0)
    } else {
        return None;
    };
    let number = number.parse::<f32>().ok()?;
    number
        .is_finite()
        .then(|| (number * scale).clamp(-5000.0, 10000.0) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cascade_handles_specificity_important_inline_and_unsupported_rules() {
        let mut css = Stylesheet::default();
        css.append("@import url(https://example.com/ignored.css); /* comment */ p {text-align:left} .verse {text-align:right} #poem {text-align:center !IMPORTANT} body .verse {white-space:pre-wrap} @media print {p {display:none}} p:hover {display:none}");
        let ancestry = vec![
            ElementStyle {
                tag: "body".into(),
                id: String::new(),
                classes: vec![],
            },
            ElementStyle {
                tag: "p".into(),
                id: "poem".into(),
                classes: vec!["verse".into()],
            },
        ];
        let properties = css.properties(&ancestry, "text-align:justify; margin-top:2em");
        assert_eq!(properties["text-align"], "center");
        assert_eq!(properties["white-space"], "pre-wrap");
        assert!(!properties.contains_key("display"));
        assert_eq!(em_length(&properties["margin-top"]), Some(2000));
        assert_eq!(em_length("NaNem"), None);
    }
}
