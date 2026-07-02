# README Asset Workflow

README images must be real Explorer screenshots, not generated mockups. The fixture and isolated settings are generated under `target/readme-assets/` so the app never uses a developer's real Downloads folder or personal Explorer configuration.

## Prepare Fixture

```sh
cargo run --example readme_assets -- prepare
```

This creates:

- `target/readme-assets/fixture/Explorer Demo`: deterministic sample files and folders.
- `target/readme-assets/config/overview`: settings for the details-view screenshot.
- `target/readme-assets/config/large-icons`: settings for the large-icons screenshot.
- `target/readme-assets/config/image-viewer`: sample image path for the image viewer screenshot.
- `target/readme-assets/window-state.json`: a stable 1200 x 820 window size.
- `target/readme-assets/captures`: staging directory for raw screenshots.

## Capture Screenshots

Use the generated settings for each screenshot scenario. On Windows, launch Explorer with `USERPROFILE` set to the scenario config directory. On Linux, launch with `XDG_CONFIG_HOME` set to the scenario config directory. On macOS, launch with `HOME` set to the generated scenario home directory.

Expected committed files:

- `docs/assets/explorer-overview.png`
- `docs/assets/large-icons.png`
- `docs/assets/image-viewer.png`

Raw captures can be staged at:

- `target/readme-assets/captures/explorer-overview.png`
- `target/readme-assets/captures/large-icons.png`
- `target/readme-assets/captures/image-viewer.png`

## Commit Optimized Images

```sh
cargo run --example readme_assets -- optimize
```

The optimize command copies staged screenshots into this directory and validates that each image is present, readable, and reasonably sized for GitHub. It does not fabricate missing screenshots.
