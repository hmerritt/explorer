<img src="./assets/explorer.png" draggable="false" width="100px" />

# Explorer Development

[![Release](https://img.shields.io/github/v/release/hmerritt/explorer)](https://github.com/hmerritt/explorer/releases/latest) [![Downloads](https://img.shields.io/github/downloads/hmerritt/explorer/total)](https://github.com/hmerritt/explorer/releases/latest) [![Coverage](https://img.shields.io/coverallsCoverage/github/hmerritt/explorer)](https://coveralls.io/github/hmerritt/explorer?branch=master)

Explorer is a Rust/GPUI desktop file manager for Windows Explorer-style workflows on macOS, Linux, and Windows.

## Development

This app currently targets Windows, macOS, and Linux. Other platforms compile a small fallback binary that prints a platform support message.

```sh
cargo check
cargo test
cargo run
```

CI and release builds use locked dependency resolution. Prefer these checks before dependency-sensitive changes:

```sh
cargo check --locked
cargo test --locked --all-targets
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

README screenshots are generated from deterministic fixture content. See
`docs/assets/README.md` and run:

```sh
cargo run --example readme_assets -- prepare
```

---

### macOS

macOS release artifacts ship `Explorer.app` so Finder and Launch Services start
Explorer as a GUI application instead of a terminal-launched executable. Release
builds generate the app icon from `assets/explorer.png`, include an `Info.plist`, and
ad-hoc sign the bundle so its local signing metadata is coherent.

For a local user install after building from source on macOS, run:

```sh
cargo build --release
./install-macos.sh
open "$HOME/Applications/Explorer.app"
```

The installer creates or updates `~/Applications/Explorer.app` by default. Set
`EXPLORER_BINARY_PATH` to install a non-default build output, or
`EXPLORER_INSTALL_DIR` to install into another user-writable directory.

A better long-term macOS distribution should use a Developer ID certificate,
notarize with Apple, staple the notarization ticket, and package as a `.dmg` or
`.app.zip`. Apple documents this flow in
[Distributing software on macOS](https://developer.apple.com/macos/distribution/),
[Developer ID](https://developer.apple.com/developer-id/), and
[Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution).

---

### Windows installer and updates

Windows releases keep two different copies of the executable: the portable ZIP
is compressed with UPX, while the Squirrel package is always built from the
uncompressed Cargo release binary. To build and validate the unsigned installer
locally from PowerShell with NuGet available on `PATH`:

```powershell
cargo build --release --locked --target x86_64-pc-windows-msvc
./scripts/windows/package-squirrel.ps1 `
  -Version "0.22.0" `
  -BinaryPath "target/x86_64-pc-windows-msvc/release/explorer.exe"
```

The script pins Squirrel.Windows 2.0.1, uses
`packaging/windows/squirrel/explorer.nuspec`, and writes the installer,
`RELEASES`, and the full package to `dist/squirrel`. It deliberately creates no
delta package or MSI. It also verifies the NuGet payload, release-manifest names,
sizes and hashes, the embedded setup payload, and known dummy-binary markers.

Release tags must be numeric and exactly match the package version in
`Cargo.toml`. A usable GitHub release must publish all four Windows assets from
the same build:

- `explorer-<version>-windows-amd64.zip`
- `explorer-<version>-windows-amd64-installer.exe`
- `RELEASES`
- `explorer-<version>-full.nupkg`

Do not publish a partial release: installed clients use the stable
`releases/latest/download` directory and require both `RELEASES` and its complete
full package. Updates install in the background and become active only after the
running Explorer process closes and the user reopens it.

Before publishing, use a disposable Windows user or VM and a controlled HTTP
feed. Install version A, publish a complete version B feed, wait for the update,
confirm A remains usable, then reopen and confirm B runs. Also verify Start Menu
and Desktop shortcuts, image **Open with** entries, first-run delay, offline and
malformed feeds, Unicode/spaced install paths, repeated checks, and uninstall.
Portable and Scoop launches must never start `Update.exe`.

---

<small>
    <a href="https://www.flaticon.com/free-icons/folder" title="folder icons">Folder icons created by kmg design - Flaticon</a>
</small>
