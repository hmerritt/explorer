# Agent Instructions

## Project Mission

Explorer is a cross-platform file explorer written in Rust with GPUI.
Its product goal is to be Windows Explorer in all user-visible behavior: UI,
workflows, keyboard and mouse interactions, and file-management semantics should
match Windows Explorer as closely as possible.

Do not redesign, simplify, or "fix" Windows Explorer behavior unless the user
explicitly asks for that. Other explorers may add features or intentionally
change the experience; this project should preserve the Windows Explorer model.

macOS and Linux are the core support systems for implementation decisions.
Windows is supported too, but when platform behavior or filesystem APIs differ,
think about robust macOS and Linux behavior first while preserving the Windows
Explorer UX model. On every platform, Windows Explorer remains the source of
truth for visible behavior and interaction design.

## Repository Orientation

- This is a Rust 2024 project using `gpui`.
- The main application entry point is `src/main.rs`.
- Window and application setup lives in `src/app.rs`.
- Explorer UI, navigation, rendering, sorting, formatting, and filesystem
  behavior are currently concentrated in `src/explorer.rs`.
- `assets/explorer.png` is the canonical app icon source. Windows `.ico` and
  resource files are derived or platform-specific assets.

## Development Commands

Use the standard Rust workflow:

```sh
cargo check
cargo test
cargo run
```

CI and release builds use locked dependency resolution. Prefer `--locked` when
validating dependency-sensitive changes, for example:

```sh
cargo check --locked
cargo test --locked --all-targets
```

## Coding Guidance

- Prefer small, focused Rust changes.
- Add or update tests for behavior-heavy Explorer details.
- Preserve the existing GPUI style, layout constants, and rendering patterns
  unless the change is intentionally improving Windows Explorer parity.
- Use observed Windows Explorer behavior as evidence for navigation, selection,
  sorting, sizing, naming, context menus, keyboard shortcuts, toolbar behavior,
  and other user-visible details.
- Keep cross-platform code guarded with `cfg` where platform APIs or behavior
  differ.
- Avoid treating README wording that describes the app as a skeleton as the full
  source of truth. Inspect the source first; explorer behavior already exists.

## Validation Guidance

- For logic changes, add or update Rust unit tests near the affected code.
- For UI behavior, manually test with `cargo run` where feasible and describe
  what was checked.
- Documentation-only changes do not require running `cargo`, but still review
  the markdown for stale commands or inaccurate project guidance.
