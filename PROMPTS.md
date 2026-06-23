## Todo

- rclone https://rclone.org/ builtin. https://crates.io/crates/librclone

## 1

- Support Google Drive when synced: Default windows location is: C:\Users\hrmer\AppData\Local\Google\Google Drive Streaming\My Drive.lnk

## 2

- Improve readme - add screenshots / branding
- Drive total size + used, GB and percentages
- UI refinement and improvements (tighten everything up, make it look nice)
- Refactor the conflict dialog for copy to include rsync-like settings (delete/keep differences, etc...)
- Special settings value for "date_format" called "relative" and another "relative-timestamp", which shows relative human-readable "ago" times, and "-timestamp" varient includes "at <%H:%M>", e.g. "1 minute ago", "2 hours ago", "yesterday at 15:29", "2 days ago"...

## 3

- Auto clear caches every so often (30 days?) - run through cached items and check their item path to see if it still exists, if it does not then delete the cached item
- Settings UI
    - context-menu
        - Detect installed programs, suggest adding into menu
- Split-screen (see Zed)
- Shell-extension system
- SSH drive support
- Google Drive, OneDrive, etc... mounting
- rclone hook for drive, B2, S3, etc...
- (maybe?) Implement a new settings item "search_recursive_max_items" for recursive search to limit the number of items returned in the view (to improve render performance)
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
- Audio metadata
    - Channels
    - Format
    - Sample Rate
- Text file
    - Lines
    - Lines of text
    - Blanks
- PDF view: https://crates.io/crates/pdf_oxide
