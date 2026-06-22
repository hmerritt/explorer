## Todo

- Video Properties > Frames tab, add a contextmenu to each frame with "Copy" to copy the image to clipboard. Also add this copy functionality to Images Properties > Image
- Windows-only: Remove "Open with" for .exe
- When in a repo directory (or sub-directory) add a new contextmenu item "Copy <file/folder> relative repo path" to copy the item path, relative to the repo root. Add this below the exiting "Copy <file/folder> path" item, with the same icon
- Add settings icon: ./assets/icons/utility/settings.svg - place the settings icon button on the lower left of the side-bar. Set opacity 0 by default and fade-in on hover to lower section of side-bar. For now, this icon opens the settings file.

## 1

- rsync copy/sync builtin
- Mount ISO (and other mountable files) as drive. Add icon to the utility bar, on the left (same position as Extract). Use icon:

## 2

- rsync copy/sync builtin
- Improve readme - add screenshots / branding
- Drive total size + used, GB and percentages
- UI refinement and improvements (tighten everything up, make it look nice)
- Refactor the conflict dialog for copy to include rsync-like settings (delete/keep differences, etc...)
- Properties "Opens with" "Change" button currently opens the file after a change. It should only set the default program, and not open the program.
- Special settings value for "date_format" called "relative" and another "relative-timestamp", which shows relative human-readable "ago" times, and "-timestamp" varient includes "at <%H:%M>", e.g. "1 minute ago", "2 hours ago", "yesterday at 15:29", "2 days ago"...

## 3

- Settings option not to hide "." items (only hidden attribute items)
- Settings UI
    - context-menu
        - Detect installed programs, suggest adding into menu
- Split-screen (see Zed)
- Shell-extension system
- SSH drive support
- Google Drive, OneDrive, etc... mounting
- Network drives (rclone https://rclone.org/ builtin)
- rclone hook for drive, B2, S3, etc...
- rclone https://rclone.org/ builtin, adding support for
- Drag-and-drop — How should Alt-drag shortcut behavior be handled in this task?
  Alt-drag should create a shortcut or simlink of the selected file/directory
- (maybe?) Implement a new settings item "search_recursive_max_items" for recursive search to limit the number of items returned in the view (to improve render performance)
- Add UI button to calc folder sizes (button disapears once pressed, it's a one-time button that must be pressed per-directory as-and-when it is needed) Only show when "show_folder_size: false"
- A new implementation detail regarding selecting items, and triggering rubber-band selection. Currenty the logic is that anywhere in the Name column won't select, but the othe columns will. There is more to be done here. Windows Explorer actually has it like this: Name column will not select, but Name column on the item text (filename/folder name) WILL select straight away. The same is true for the rubber-band. If I drag on an item Name text, it will drag straight away, whereas on the Name column but not the text won't

## Left to implement

Major remaining Windows Explorer parity areas:

2. **View modes and folder presentation**
   The app is mostly one Details-style list. Still missing large/medium/small icons, tiles, content view, list view, grouping, column resizing/reordering/choosing, sort direction UI, per-folder view persistence, preview pane, details pane, and thumbnail generation.

3. **Context menus and shell verbs**
   No full right-click model yet: Properties, Copy as path, Send to, Share, Pin, New item templates, terminal/open here, app-specific verbs, and empty-folder/background context menus.

4. **Navigation shell surfaces**
   Sidebar is currently basic user dirs/drives plus macOS-specific entries. Remaining: This PC-style device grouping, Network, removable/media volumes, trash/recycle bin browsing, sidebar tree expansion.

5. **File operation parity**
   Current copy/move/delete is strong, but Explorer has much more: pause/resume, speed/ETA details, recycle bin restore/empty, robust cross-app clipboard formats, shortcut creation via Alt-drag, and more exact same-volume/network behavior.

6. **Keyboard and mouse completeness**
   Implemented keys cover common navigation and rename, but Explorer has a large set left: context-menu key/Shift+F10, F10/menu behavior, Ctrl+N new window.

7. **Shell integration and platform associations**
   Opening files uses the default app, but there is no full file association management, executable/app launching nuance, shortcut/link creation/editing, mounted volume eject, network path handling, or platform-native trash/recycle-bin browsing.

## Properties > Details tab:

- Image metadata
    - Rotate images Left/Right
    - Edit metadata values
- Video metadata
    - Frames tab, add a context menu to each frame with:
        - Copy
        - Save
- Audio metadata
    - Channels
    - Format
    - Sample Rate
- Text file
    - Lines
    - Lines of text
    - Blanks
- Code file
    - https://crates.io/crates/tokei
    - https://docs.rs/tokei/latest/tokei/index.html
    - Language
    - Lines
    - Lines of Code
    - Blanks
- PDF view: https://crates.io/crates/pdf_oxide
