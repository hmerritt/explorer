## 1

- Additional binary feature of viewing images. I want to create an embeded image viewer, as an alternative UI to the explorer (still with GPUI). So if a user (or the program itself) calls the binary along with an image path, it will open this image UI - "explorer.exe <path-to-image>" if the path exists, open the image UI, if it does NOT exist, ignore and open existing explorer UI.
- Use an alternate render path high-up in the program, add this new UI into src/image/*
- A window with the image rendered in the centre. Fit within the window size (same as Image properties tab behaviour)
- Scaling should be high-quality (Lanczos or equivalent)
- Colour profile (if exists) should be read and applied
- Window should use inline OS controls for minimise, maximise, close (as main explorer app does). Image filename should also be rendered at the left-top (inline with controls)

---

- A bottom status bar with resolution, current scaling (as a percentage), size, decompressed size
- Refactor image scaling so that the image 

## 2

- Improve readme - add screenshots / branding
- Drive total size + used, GB and percentages
- UI refinement and improvements (tighten everything up, make it look nice)
- Refactor the conflict dialog for copy to include rsync-like settings (delete/keep differences, etc...)
- Support Google Drive when synced: Default windows location is: C:\Users\hrmer\AppData\Local\Google\Google Drive Streaming\My Drive.lnk

## 3

- Settings UI
    - context-menu
        - Detect installed programs, suggest adding into menu
- Split-screen (see Zed)
- Shell-extension system
- SSH drive support
- Spectrum analyser https://www.spek.cc/about
- Google Drive, OneDrive, etc... mounting
- (maybe?) Implement a new settings item "search_recursive_max_items" for recursive search to limit the number of items returned in the view (to improve render performance)
- Special settings value for "date_format" called "relative" and another "relative-timestamp", which shows relative human-readable "ago" times, and "-timestamp" varient includes "at <%H:%M>", e.g. "1 minute ago", "2 hours ago", "yesterday at 15:29", "2 days ago"...
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
- Text file
    - Lines
    - Lines of text
    - Blanks
- CSV
- JSON
- PDF view: https://crates.io/crates/pdf_oxide
- EPUB: https://crates.io/crates/rbook
