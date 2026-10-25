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
