<img src="./assets/explorer.png" draggable="false" width="100px" />

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

On Linux, Explorer is GUI-only and requires either X11 or Wayland at startup.
X11 is preferred when `DISPLAY` is set because GPUI 0.2.2 depends on the Wayland
compositor for server-side titlebar decorations, and not all compositors provide
them. If X11 is unavailable but `WAYLAND_DISPLAY` points to a connectable socket,
or `$XDG_RUNTIME_DIR/wayland-0` is available, Explorer starts on Wayland. Set
`EXPLORER_LINUX_BACKEND=auto`, `EXPLORER_LINUX_BACKEND=wayland`, or
`EXPLORER_LINUX_BACKEND=x11` to choose backend selection explicitly; requested
unavailable `wayland` or `x11` backends fail startup instead of falling back. If
neither backend is available, startup exits with a fatal error.

The canonical app icon source is `assets/explorer.png`. It is referenced as package/bundle metadata in `Cargo.toml` for tooling that understands `[package.metadata.bundle]`; `assets/explorer.ico` is a derived Windows executable resource.

---

### macOS

macOS release artifacts ship `Explorer.app` so Finder and Launch Services start
Explorer as a GUI application instead of a terminal-launched executable. Release
builds generate the app icon from `assets/explorer.png`, include an `Info.plist`, and
ad-hoc sign the bundle so its local signing metadata is coherent.

A better long-term macOS distribution should use a Developer ID certificate,
notarize with Apple, staple the notarization ticket, and package as a `.dmg` or
`.app.zip`. Apple documents this flow in
[Distributing software on macOS](https://developer.apple.com/macos/distribution/),
[Developer ID](https://developer.apple.com/developer-id/), and
[Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution).

---

<small>
    <a href="https://www.flaticon.com/free-icons/folder" title="folder icons">Folder icons created by kmg design - Flaticon</a>
</small>
