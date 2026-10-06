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

Self-host open-source Archivo, Inter and Geist Mono fonts with their licenses.
Preserve readable text contrast, keyboard focus, and reduced-motion preferences.
Screen captures use the isolated README demo fixture; no personal file listings.
