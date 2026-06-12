## New

Debug relative timings - implement a time-since-last-log so each print you can print, +30ms, before the total time.

Like the existing total time, also pad out to prevent layout shifts.

Refactor how the padding works for both, by putting the measurment as the final chars in the padding "34.098 ms"

```
[nav] 2.000µs    navigate.start from="C:\\Users\\hrmer\\Downloads" to="C:\\Users\\hrmer\\Downloads\\archives" history=Record same_path=false
[nav] 61.300µs   navigate.pre_reload from="C:\\Users\\hrmer\\Downloads" to="C:\\Users\\hrmer\\Downloads\\archives" history=Record
[nav] 1.100µs    reload.selected_paths path="C:\\Users\\hrmer\\Downloads\\archives" selected=0
[nav] 296.900µs  reload.sidebar_sections path="C:\\Users\\hrmer\\Downloads\\archives"
[nav] 43.300µs   load_entries.read_dir path="C:\\Users\\hrmer\\Downloads\\archives" ok=true
[nav] 68.200µs   load_entries.filter path="C:\\Users\\hrmer\\Downloads\\archives" scanned=3 hidden=0 entry_errors=0
[nav] 1.629s     load_entries.materialize path="C:\\Users\\hrmer\\Downloads\\archives" entries=3 skipped=0
[nav] 1.634s     load_entries.scan path="C:\\Users\\hrmer\\Downloads\\archives" scanned=3 entries=3
[nav] 16.100µs   load_entries.sort path="C:\\Users\\hrmer\\Downloads\\archives" entries=3
[nav] 1.640s     load_entries.total path="C:\\Users\\hrmer\\Downloads\\archives" entries=3 show_hidden=false
[nav] 1.643s     reload.load_entries path="C:\\Users\\hrmer\\Downloads\\archives" ok=true entries=3
[nav] 21.000µs   reload.search_filter path="C:\\Users\\hrmer\\Downloads\\archives" query="" visible=3 selected=0
[nav] 1.652s     reload.total path="C:\\Users\\hrmer\\Downloads\\archives" entries=3 all_entries=3 read_error=false
[nav] 1.654s     navigate.reload same_path=false path="C:\\Users\\hrmer\\Downloads\\archives"
[nav] 531.300µs  watcher.restart path="C:\\Users\\hrmer\\Downloads\\archives" ok=true
[nav] 1.669s     navigate.total from="C:\\Users\\hrmer\\Downloads" to="C:\\Users\\hrmer\\Downloads\\archives" same_path=false entries=3 read_error=false
```

## Specific / for-later

- Alt-double-click opens a "Details" window, for both files/folders.
- Left side-bar drag re-sizable
- Drag-and-drop — How should Alt-drag shortcut behavior be handled in this task?
  Alt-drag should create a shortcut or simlink of the selected file/directory
- When navigating to a directory, load in a non-blocking way. Keep the view the same initially, but if after 100ms it is still loading, preload the UI in the directory and display a loading spinner until completion. Since this is happening in a non-blocking thread, a user can choose to navigate away, in which case when the thread returns the result is simpily discarded
- Progress dialogue when moving files around. Perform async, only show dialogue if operation takes longer than 500ms

## Ideas

- Split-screen (Zed style)
- Shell-extension system
- rsync copy/sync builtin
- Customizable context menus (add items to settings)
- Network drives (rclone https://rclone.org/ builtin)
- rclone https://rclone.org/ builtin, adding support for

- Context menu
- File icons
- Large icons grid view (alternate to the current Details view)
- Drag file view headers width + ordering (not Name, keep this dynamic, fixed first)
- ssh drive support
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

## Left to implement

Major remaining Windows Explorer parity areas:

2. **View modes and folder presentation**
   The app is mostly one Details-style list. Still missing large/medium/small icons, tiles, content view, list view, grouping, column resizing/reordering/choosing, sort direction UI, per-folder view persistence, preview pane, details pane, and thumbnail generation.

3. **Context menus and shell verbs**
   No full right-click model yet: Open with, Properties, Copy as path, Send to, Share, Pin, New item templates, terminal/open here, app-specific verbs, and empty-folder/background context menus.

4. **Properties and metadata**
   Missing file/folder Properties dialogs, attributes, permissions, owner/group/security, timestamps editing, size on disk, target details for links, file type association details, and richer media/document metadata columns.

5. **Navigation shell surfaces**
   Sidebar is currently basic user dirs/drives plus macOS-specific entries. Remaining: This PC-style device grouping, Network, removable/media volumes, trash/recycle bin browsing, sidebar tree expansion.

6. **File operation parity**
   Current copy/move/delete is strong, but Explorer has much more: pause/resume, speed/ETA details, recycle bin restore/empty, robust cross-app clipboard formats, shortcut creation via Alt-drag, and more exact same-volume/network behavior.

7. **Keyboard and mouse completeness**
   Implemented keys cover common navigation and rename, but Explorer has a large set left: Alt+Enter properties, context-menu key/Shift+F10, F10/menu behavior, Ctrl+N new window.

8. **Shell integration and platform associations**
   Opening files uses the default app, but there is no full file association management, “Open with” picker, executable/app launching nuance, shortcut/link creation/editing, mounted volume eject, network path handling, or platform-native trash/recycle-bin browsing.
