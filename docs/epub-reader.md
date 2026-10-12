# EPUB reader

Explorer's reader uses local EPUB files and native GPUI pagination. The three
quality milestones extend the existing renderer and persistence format.

## Milestone 1: content and reliability

- Scene separators, explicit verse line breaks, preformatted text, monospace
  code, raised/lowered superscript and subscript markers, and marked note links.
- Tables retain rows, cells, empty cells, headers, and column spans. Rows that
  fit are rendered as a grid. Oversized rows flow as individual cells across
  pages to avoid dropping text.
- Inline styles and linked stylesheets inside the archive support simple
  tag/class/ID selectors, compound selectors, descendant selectors, specificity,
  source order, inline overrides, and `!important`.
- Supported properties: `text-align`, `text-indent`, `margin-top`,
  `margin-bottom`, `white-space`, `display: none`, `visibility: hidden/collapse`,
  `break-before/after`, and `page-break-before/after` at text block boundaries.
  Lengths support `em`, `px`, `pt`, and zero; pixel/point lengths are normalized
  against a 20px base and scale with the reader font size.
- Book identity uses a streamed SHA-256 digest of the archive on a worker.
  Byte-identical files share progress and bookmarks after moves, renames, or
  timestamp changes. Repacking or editing the archive creates a new identity.
  Legacy path-based positions migrate when the original fingerprint still
  matches; open an unchanged original once before moving it to migrate.
- Section failures identify the section and provide Retry, Back, and Skip.
  Retry reloads that section; persistent malformed markup still needs a
  repaired EPUB. Fixed-layout and encrypted text produce explicit messages.

## Milestone 2: comfort and navigation

Reading settings offer serif/sans-serif, 12–36px font size, 1.2–2.2 line
spacing, 400–1000px maximum reading width, left/justified alignment, theme,
publisher layout overrides, and reduced motion. Existing saved font size and
theme remain intact; missing settings use the original defaults (20px serif,
1.5 spacing, 900px maximum width, justified body text, dark theme).

Publisher layout controls alignment, indentation, paragraph spacing, and page
breaks. Font family, font size, and line spacing always come from reader
settings. Structural whitespace and hidden content remain respected when
publisher layout overrides are disabled. Justification leaves a line at natural
spacing when expansion would exceed 0.6em per word space.

The toolbar uses the current applicable Contents heading, local page number,
and approximate whole-book progress based on visible text character counts.
Chapter page totals appear when measured pagination is complete. Covers and
supplements do not count toward whole-book progress. Search/progress indexing
runs in the background; progress is independent of typography. If sections
cannot be parsed, the percentage reflects the successfully indexed text.

Contents highlights the current heading, scrolls it into view, supports
Up/Down/Home/End/Enter, and closes with Escape or an outside click. Back restores
the passage preceding a Contents, bookmark, or internal link jump. Controls have
Tab focus, visible focus states, Enter/Space activation, and shortcut tooltips.

## Milestone 3: returning to passages

- Whole-book case-insensitive Unicode search runs on workers with cancellation
  and a short typing debounce. Results include section titles and excerpts;
  selecting a result highlights its range. Enter chooses the first result.
  Return to reading restores the passage where search began. At most 200
  matches are displayed. Linked readable supplements are included.
- Named bookmarks appear in Contents and can be removed there. Locations include
  surrounding text and use it to recover anchors after parser changes, falling
  back to a valid content offset if the excerpt cannot be found.
- Marked EPUB note references open a dismissible preview of the note's complete
  text without navigating away, including notes hidden from the main reading
  flow. Other internal links navigate with Back history.
- Double-click selects a word; triple-click selects a text block. Drag selection,
  copy shortcuts, and a selection Copy context menu use semantic UTF-8 ranges.
- F11 toggles fullscreen with hidden chrome. Moving the pointer near the bottom
  reveals controls; shortcuts open panels while fullscreen. Escape exits
  fullscreen after dismissing active overlays or selections.
- Double-click an image to enlarge it; Close or Escape returns to the passage.
- Reduced motion removes the animated loading indicator. Page changes and
  panels are immediate. This explicit preference is persisted; automatic
  operating-system reduced-motion detection is not available in this renderer.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Next / previous page | Right / Left, Page Down / Page Up, Space / Shift+Space |
| Beginning / end of book | Home / End |
| Back after a jump | Alt+Left |
| Contents and bookmarks | Ctrl+T / Cmd+T |
| Reading settings | Ctrl+, / Cmd+, |
| Search | Ctrl+F / Cmd+F |
| Add a bookmark | Ctrl+D / Cmd+D |
| Copy selection | Ctrl+C / Cmd+C |
| Fullscreen | F11 |
| Focus next / previous control | Tab / Shift+Tab |
| Activate focused control | Enter / Space |
| Dismiss panel, preview, or selection | Escape |

Native text inputs support selection, clipboard, undo/redo, and IME composition.
Page shortcuts yield to input editing and focused control activation.

## Compatibility and accessibility limits

This is a defined subset of EPUB formatting, not full EPUB reading-system
conformance. Embedded publisher fonts, arbitrary CSS, media queries, imports,
complex selectors, vertical writing, fixed layout, and scripts are unsupported.
External styles/images are not fetched. Tables currently lack row spans, nested
table layout, and full CSS border/sizing behavior; text inside cells retains
paragraph breaks, but images in cells can become separate content blocks.
Preformatted text wraps to keep it readable in narrow windows. Screen-reader
exposure and native bidirectional/vertical-layout behavior need further work.

The page is a custom canvas containing shaped glyphs. The vendored GPUI source
currently exposes no semantic accessibility tree or platform bridge for these
passages. Keyboard access and native text input improve operation but do not
make the book text accessible to NVDA, VoiceOver, or Orca. A separate platform
accessibility implementation must expose document order, headings, links,
selection, images, and table/note relationships before claiming accessible
reading. Font fallback alone is not proof of full mixed-language support.

## Validation

Regression fixtures are generated inside tests for scene breaks, poetry,
marked notes, hidden content, empty/spanned/oversized table cells, long chapters,
Unicode text, content identity, and recovered passage anchors. GPUI interaction
tests cover native input and key dispatch, selection/link hit testing, resized
pages, typography changes, image loading, Contents, search return, bookmarks,
note previews, fullscreen state, and section error recovery.

Run:

```sh
cargo check --locked
cargo test --locked --all-targets
cargo test --locked epub_reader -- --test-threads=1
cargo run --locked -- /path/to/book.epub
```

The CI platform matrix includes EPUB tests on macOS, Linux, and Windows.
Automated GPUI tests do not replace live usability validation. Before release,
manually check each platform with narrow and large windows, high DPI, mouse and
trackpad paging, keyboard-only use, IME input, fullscreen, and real publisher
books. Resize or alter typography after opening images, searching, or jumping
through Contents, and confirm that the intended passage stays visible. Test
NVDA, VoiceOver, and Orca separately when an accessibility bridge is added.
