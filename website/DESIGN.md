# Explorer website design

Reference: https://getartcraft.com/apps/photocraft, inspected 6 October 2026.
Evidence: live rendered DOM measurements at 1280 × 720, desktop screenshots,
and Firecrawl branding/content extraction. Reference assets are not reused.

## Measured reference

- Light background `#F2F1EE`; ink `#101014`; strong headings black. Muted text
  uses ink at 62% opacity, secondary labels 42%. Blue accents mark emphasis.
- Page frame: 1280px maximum width, centered, 1px vertical borders. Section
  captions have 40px horizontal padding and approximately 44px height.
- Fixed header: 49px high, 1px dividing rules, monospaced uppercase links.
- Archivo display headings: desktop h1 72px / 1.02, weight 620, tracking
  -0.035em, maximum width 896px. H2 60px / 1.02. H3 24px / 32px,
  tracking -0.02em. Body Inter 16px / 24px; hero copy 18px / 29.25px,
  maximum width 576px. Geist Mono labels 11px / 16.5px, weight 500.
- Hero icon 112px square; top of icon 171.5px from the viewport top.
  Hero h1 starts at 359.5px. The centered hero has generous vertical space.
- Buttons are square, 48px high, 24px horizontal padding, uppercase 11px
  monospaced labels; primary ink fill, secondary transparent with a border.
- Highlights: three columns, two rows, shared 1px borders, 32px card padding;
  numbered labels precede headings with 24px spacing.
- Hero screenshot frame 16:9. Gallery: first screenshot spans both columns,
  followed by two equal-width images. Gallery frames 16:10; caption strips
  approximately 43px high. Images open at full size.
- Downloads: three bordered columns; release/source area split in two.
- Fixed right-hand section links and a thin viewport-edge scroll ruler.

The section tracker follows scroll position: upcoming labels queue at the
bottom-right, rotate letter by letter onto the vertical rail, then flip into a
top stack. The current heading is 34px; passed/queued labels are 16px and riding
labels 20px. Motion reverses on upward scrolling without automatically nudging
the page. The 60px ruler has document-based 1% ticks, labels every 5%, and a blue
needle with a rolling three-digit percentage. Desktop fine-pointer devices show
the tracker; reduced motion uses static links and instant digit updates. Anchor
links retain native navigation. Geometry is refreshed after fonts load and when
the content or viewport changes.

## Explorer adaptation

Keep the reference geometry, font families and light palette. Replace the logo,
copy and media with real Explorer assets. Use a blue accent `#2467D1` (visual
approximation). Omit the product-family section and dark-theme control. Renumber
the contribution section to 05. Header/footer destinations must be real Explorer
links. No unrelated ArtCraft destinations or borrowed screenshots.

At widths below 1024px the display heading is 60px; below 768px 48px; below
640px 36px. Stack grids on narrow screens, use a collapsible header menu, hide
the fixed section navigator, and reduce outer padding to 20px. Responsive values
are verified against the reference during browser validation.

Self-host open-source Archivo, Inter, Geist Mono and Instrument Serif fonts with
their licenses. Archivo supports weights 100–900 and widths 62–125%; its
Arial-based fallback uses the reference's metric overrides (88.96% ascent,
21.28% descent, zero line gap, 98.7% size adjustment).

Instrument Serif's regular and italic faces are registered with the reference's
Times New Roman fallback and available through `.font-serif`. PhotoCraft's app
page currently uses Archivo for every display heading and does not render any
Instrument Serif text; keep the same placement on Explorer's app page.
Preserve readable text contrast, keyboard focus, and reduced-motion preferences.
Screen captures use the isolated README demo fixture; no personal file listings.
