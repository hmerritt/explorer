# Recycle Bin / Trash

The sidebar location after Pinned displays the current user's system bin,
including items deleted by other applications. Its canonical address is
`trash:///`. Windows calls it Recycle Bin; macOS and Linux call it Trash.

The location works in tabs, split panes, and navigation history. Details is the
initial view, sorted by name ascending. Bin view, column, and sort preferences
are kept separately from ordinary folders during the session. Search matches
names or original locations. Enter and double-click display deletion properties;
deleted folders cannot be entered.

The bin toolbar contains Cut and Delete icons, the standard View dropdown,
Restore, Restore to…, and Empty Recycle Bin / Empty Trash, in that order with
separators between groups. Properties remains available through the context menu,
Enter, and double-click. There is no Restore all items command.

Bin dialogs fit their content. Properties grows up to a bounded height and then
scrolls; progress filenames are truncated so long names do not resize the window.

Restore returns items to their recorded original locations and recreates missing
parents. Restore to… chooses a local or mounted folder. Cut/paste and drag-out
within Explorer use the same recovery pipeline. Copy and link recovery, paste
into the bin, remote-server recovery, and external-app bin clipboard/drag
interoperability are unavailable.

Conflicts support Replace, Skip, Keep both, and Cancel. Skipped or failed source
contents remain recoverable. Recovery verifies staged destination contents before
removing source data. Successful recovery is undoable; undo checks destination
filesystem identity, returns recovered contents to the system bin, and restores
replacement backups. Undo history is limited to the current Explorer session.

Delete, Shift+Delete, and Empty request irreversible deletion confirmation.
Empty captures the entire bin identity set before confirmation, including items
hidden by search. Items arriving after confirmation preparation survive. Neither
permanent deletion nor Empty creates undo entries.

App mutations are serialized across windows. Open bin views refresh after app
operations, activation, Refresh/F5, and every five seconds while visible. Every
mutation resolves bin identities again; original paths are destinations only.

Empty and selected-item permanent deletion resolve the confirmed identities from
one fresh listing and refresh once after completion or cancellation. Windows
queues those items in one Shell operation, retaining per-item outcomes and
checking payload identity immediately before deletion. Cancel stops pending
operations; an identity change retains the replacement and resumes untouched
items in a new Shell operation. Items arriving after confirmation remain intact.

## Platform behavior

Linux uses the trash crate's Freedesktop enumeration and native ordinary restore
and purge support, with verified transfers for alternate destinations and merges.
Accessible mounted-volume bins are included. Windows runs shell operations on
COM workers and checks both per-item outcomes and aborted operations.

macOS enumerates home and mounted-volume Trash directories. Explorer records
Foundation's resulting URL, original path, deletion time, volume, and filesystem
identity in an atomic `trash-origins.json` journal alongside its settings. Items
without trustworthy origin information display Unknown and can be recovered with
Restore to…. Original-location recovery is guaranteed for tracked Explorer
deletions; Finder and other applications do not supply reliable origins.

## Validation

Run `cargo check --locked` and `cargo test --locked --all-targets`. Recovery and
purge tests use fake backends and temporary files. Normal automated tests never
empty the user's system bin. The ignored native round-trip smoke test creates and
recovers disposable files and a folder, and purges one owned fixture only:

```sh
cargo test --locked --lib native_bin_disposable_file_roundtrip -- --ignored --test-threads=1 --nocapture
```

On Windows, this additional disposable test exercises file and folder recovery
with ordinary, forward-slash, and canonicalized destination paths, including
skip conflicts and undo:

```sh
cargo test --locked --lib native_bin_paste_destination_formats -- --ignored --test-threads=1 --nocapture
```

Windows batch deletion checks use only owned fixtures. The callback test uses
temporary files to exercise cancellation, locked-file failure, and replacement
identity handling. The performance test moves 16 baseline files, 256 batch
files, a nested folder, and an unselected sentinel into the bin; it purges only
their explicit identities and verifies a single batch Shell execution:

```sh
cargo test --locked --lib native_batch_cancellation_and_identity_changes -- --ignored --test-threads=1 --nocapture
cargo test --locked --lib native_bin_batch_purge_performance -- --ignored --test-threads=1 --nocapture
```

On this Windows workspace, the original purge loop took 34.05 seconds for 16
files (2,128 ms/item), while the batch path took 3.52 seconds for 257 items
(13.7 ms/item), including a nested folder. The unselected sentinel survived and
progress reached 257 of 257. Timings include enumeration and final refresh;
the differing fixture counts make these per-item figures indicative rather
than a same-size benchmark.

Batch purge validation: Windows `cargo check --locked` and the serial
`cargo test --locked --all-targets -- --test-threads=1` pass (2,183 tests;
six native tests remain ignored in the standard suite). Both new native tests
pass when explicitly run. Linux (WSL) cross-target checking and all 37 focused
Trash tests pass. GPUI tests cover dialog progress and cancellation; manual
visual checks and native macOS validation have not been repeated for this change.

Before a release, use disposable files to manually exercise each platform:

- Delete in Explorer and another application; refresh and inspect metadata.
- Navigate through sidebar/address/history/tabs/split panes; test Details and icons.
- Restore known origins with missing parents and all four conflict choices.
- Recover unknown-origin items with Restore to…; try local and mounted folders.
- Cut/paste and drag between tabs and windows; reject copy/link modifiers.
- Undo recovery, including replacement backups; replace a destination independently
  and verify undo refuses to move that replacement.
- Cancel a large recovery and a purge; check successful and retained items.
- Filter the bin, confirm Empty, add another item, and verify only the confirmed
  snapshot is purged. Use an isolated account/bin for an Empty test.
- Disconnect/reconnect a mounted volume; verify unavailable-volume feedback and
  journal reconciliation. On macOS, check privacy permissions for home Trash.

Windows and Linux builds and fixture tests can be validated on this workspace;
macOS native UI and mounted-volume workflows require a macOS host.

Workspace validation for this implementation:

- Windows: `cargo check --locked` passes; all 2,140 tests pass with
  `cargo test --locked --all-targets -- --test-threads=1`. A parallel test run
  terminated with a native access violation, so the serial result is the verified
  full-suite result.
- Linux (WSL): `cargo check --locked` and all 37 focused Trash tests pass. The
  full suite has three portable-device failures because this WSL environment
  lacks `/sys/bus/usb/devices`.
- The disposable native round trip passes on Windows and Linux, including
  discovery of an item deleted outside Explorer.
- macOS backend cross-target type checking passes. Native UI, mounted-volume,
  and cross-window drag workflows still need manual verification on their
  respective platforms.

Validation for the toolbar, compact dialogs, and Windows Shell path normalization:

- Windows: `cargo check --locked` and all 2,145 non-ignored tests pass with
  `cargo test --locked --all-targets -- --test-threads=1`. Both disposable native
  recovery tests pass. The destination-format test reproduced `0x80070057` for
  forward-slash paths before the normalization change and passes afterward.
- Linux (WSL): `cargo check --locked --target x86_64-unknown-linux-gnu` and all
  39 focused Trash tests pass (one disposable native test remains ignored).
- GPUI tests check toolbar order, View menu placement at wide and narrow window
  sizes, disabled selection actions, dialog button bounds, long properties text,
  progress filename truncation, proportional progress fill, and cancellation.
- Windows `cargo run`: visually checked the toolbar, Properties, permanent-delete
  confirmation, restore-conflict choices, and compact progress window. Cut/paste
  recovered a disposable file; Restore with Skip retained the source and existing
  destination, then Keep both recovered a numbered copy without changing the
  existing destination. All manual fixtures were cleaned up.
- macOS native UI remains unverified on this Windows host.
