## New

macOS-specific features:

- A new left side-bar segment group (above the local drives), with macOS "Applications" folder in the left side-bar (load in the actual icons too), And also macOS "Bin" folder
    1. Applications
    2. Bin

---

Progress dialogue when moving files around. Perform async, only show dialogue if operation takes longer than 500ms

---

Utility bar underneeth navigation bar. Same as Windows Explorer, this has:

- New
- Cut
- Copy
- Paste
- Rename
- Delete
- View (dropdown)
    - Show hidden files
    - File Name extensions
- Select All
- Select None
- Invert Selection

## Specific / for-later

Left side-bar drag re-sizable

---

Drag-and-drop — How should Alt-drag shortcut behavior be handled in this task?

Alt-drag should create a shortcut or simlink of the selected file/directory

---

When navigating to a directory, load in a non-blocking way. Keep the view the same initially, but if after 100ms it is still loading, preload the UI in the directory and display a loading spinner until completion. Since this is happening in a non-blocking thread, a user can choose to navigate away, in which case when the thread returns the result is simpily discarded

## Didn't work first try

Signed app bundle for macOS

---

Previously — Native outbound OS dragging is explicitly staged later, per the final scope choice, because GPUI 0.2.2 does not expose the needed cross-platform drag-source/effect APIs without a GPUI patch.
Drag-and-drop — Should the plan include a local GPUI patch/vendor to expose native file drag/drop effects?
Vendor GPUI for native OS behaviour

## Ideas

- Split-screen (Zed style)
- Shell-extension system
- 7zip built-in
- rsync copy/sync builtin
- Network drives (rclone builtin)

- Context menu
- File icons
- File search, plus recursive search
- Large icons grid view (alternate to the current Details view)
- Drag file view headers width + ordering (not Name, keep this dynamic, fixed first)
- ssh drive support
- rsync for basic file operations?
- OS level hook for file operations?
- rclone hook for drive, B2, S3, etc...

## File move/copy/sync engine

For Explorer-style local move/copy, rolling checksums are usually not the best first investment. They help when the destination already has a similar file and you want to transfer only changed blocks. They do not make a normal first-time copy more reliable, and they can make local disk-to-disk copies slower because you read both source and destination heavily before writing.

Where rsync’s model is valuable here:

Reliability: resumable jobs, temp files, verification, metadata preservation, clear failure states.
Syncing folders: compare source/destination trees and update only what changed.
Interrupted operations: continue partial copies without starting over.
Large changed files: update only changed chunks when destination already exists.
For this app, I’d treat it as three layers:

Robust copy/move engine
This is highly feasible and should come first.

Copy to a temp destination file.
Flush/sync data where appropriate.
Preserve timestamps, permissions, symlinks, and directory structure.
Rename temp file into place only after success.
Report byte progress.
On failure, leave recoverable state.
Resumable sync/copy
Also feasible, medium complexity.

Keep a small operation journal.
If a partial temp file exists, resume from the last verified byte/chunk.
Verify completed chunks or whole files.
Use size/mtime first, then hash when needed.
Rolling-checksum delta copy
Feasible, but high complexity and not always beneficial.

Destination is split into blocks.
Compute weak rolling checksum plus strong hash per block.
Scan source and emit “reuse destination block” or “write literal bytes.”
Best for sync/update scenarios, not ordinary Explorer copy.
