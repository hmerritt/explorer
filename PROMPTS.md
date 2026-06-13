## New

## Ideas

## 1

- Settings options for:
    - "font" - change main app font used

- “Open with” picker
- Dialog button improvements
    - Focus state: Blue border, with an additional dashed black border inset 1px
- File/folder Properties dialogs, attributes, permissions, owner/group/security, timestamps editing, size on disk, target details for links, file type association details, and richer media/document metadata columns.
- Debug and speed up archive extraction
- Context menu for files needs to inherit the os native icon (multiselect can use the generic one)
- Improve readme - add screenshots / branding

## 2

- Organise the settings file (group related items)
- Customizable context menus (add items to settings)
- [Windows-only] detext WSL Linux install, show drive for it
- Large icons grid view (alternate to the current Details view)
- UI refinement and improvements (tighten everything up, make it look nice)
- Drag file view headers width + ordering (not Name, keep this dynamic, fixed first)
- Git support (see Files: https://github.com/files-community/Files/blob/main/.github/assets/ReadmeHero.png) (maybe implement fter shell-extension system, as a good first usecase)

## 3

- Settings UI
- Split-screen (see Zed)
- rsync copy/sync builtin
- Shell-extension system
- SSH drive support
- Google Drive, OneDrive, etc... mounting
- Network drives (rclone https://rclone.org/ builtin)
- rclone hook for drive, B2, S3, etc...
- rclone https://rclone.org/ builtin, adding support for
- Alt-double-click opens a "Details" window, for both files/folders.
- Drag-and-drop — How should Alt-drag shortcut behavior be handled in this task?
  Alt-drag should create a shortcut or simlink of the selected file/directory
- (maybe?) Implement a new settings item "search_recursive_max_items" for recursive search to limit the number of items returned in the view (to improve render performance)
- Add UI button to calc folder sizes (button disapears once pressed, it's a one-time button that must be pressed per-directory as-and-when it is needed) Only show when "show_folder_size: false"

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
