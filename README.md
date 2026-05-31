<img src="./assets/icon.png" draggable="false" width="100px" />

# Universal Explorer

[![Release](https://img.shields.io/github/v/release/hmerritt/universal-explorer?link=https%3A%2F%2Fgithub.com%2Fhmerritt%2Funiversal-explorer%2Freleases%2Flatest)](https://github.com/hmerritt/universal-explorer/releases/latest) [![Downloads](https://img.shields.io/github/downloads/hmerritt/universal-explorer/total?link=https%3A%2F%2Fgithub.com%2Fhmerritt%2Funiversal-explorer%2Freleases%2Flatest)](https://github.com/hmerritt/universal-explorer/releases/latest) [![Coverage](https://img.shields.io/coverallsCoverage/github/hmerritt/universal-explorer)](https://coveralls.io/github/hmerritt/universal-explorer?branch=master)

~Windows~ Universal File Explorer for macOS & Linux, built with [GPUI](https://www.gpui.rs/).

- [Features](#features-)
- [Development](#development-)
- [Download](#download-)

## Features ⚡

- [x] GPU accelerated file explorer
- [ ] Tabs
- [ ] Copy/Cut/Paste
- [ ] Archive compress/decompress support

## Development 🛠️

This repository is currently a Rust/GPUI application skeleton with a stub window. The real file explorer UI and filesystem browsing behavior are intentionally not implemented yet.

GPUI currently targets macOS and Linux for this app. Unsupported platforms compile a small fallback binary that prints a platform support message.

```sh
cargo check
cargo test
cargo run
```

The app icon source is `assets/icon.png`. It is referenced as package/bundle metadata in `Cargo.toml` for tooling that understands `[package.metadata.bundle]`.

## Download 💾

#### [➡️ Manually Download The Latest Release Here](https://github.com/hmerritt/universal-explorer/releases/latest)

Or via one of the supported package managers:

#### ➡️ macOS / Linux via [Homebrew](https://brew.sh/)

```sh
brew install hmerritt/tap/universal-explorer
```

---

<small>
    <a href="https://www.flaticon.com/free-icons/folder" title="folder icons">Folder icons created by kmg design - Flaticon</a>
</small>
