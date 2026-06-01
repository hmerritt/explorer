<img src="./assets/icon.png" draggable="false" width="100px" />

# Explorer

[![Release](https://img.shields.io/github/v/release/hmerritt/explorer?link=https%3A%2F%2Fgithub.com%2Fhmerritt%explorer%2Freleases%2Flatest)](https://github.com/hmerritt/explorer/releases/latest) [![Downloads](https://img.shields.io/github/downloads/hmerritt/explorer/total?link=https%3A%2F%2Fgithub.com%2Fhmerritt%explorer%2Freleases%2Flatest)](https://github.com/hmerritt/explorer/releases/latest) [![Coverage](https://img.shields.io/coverallsCoverage/github/hmerritt/explorer)](https://coveralls.io/github/hmerritt/explorer?branch=master)

File Explorer for Windows, macOS, and Linux, built with [GPUI](https://www.gpui.rs/).

## Development 🛠️

This repository is currently a Rust/GPUI application skeleton with a stub window. The real file explorer UI and filesystem browsing behavior are intentionally not implemented yet.

This app currently targets Windows, macOS, and Linux. Other platforms compile a small fallback binary that prints a platform support message.

```sh
cargo check
cargo test
cargo run
```

The canonical app icon source is `assets/icon.png`. It is referenced as package/bundle metadata in `Cargo.toml` for tooling that understands `[package.metadata.bundle]`; `assets/icon.ico` is a derived Windows executable resource.

---

<small>
    <a href="https://www.flaticon.com/free-icons/folder" title="folder icons">Folder icons created by kmg design - Flaticon</a>
</small>
