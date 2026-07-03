## 1

- Improve readme - add screenshots / branding

## 2

- Image viewer tweaks
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
- Google Drive, OneDrive, etc... mounting
- (maybe?) Implement a new settings item "search_recursive_max_items" for recursive search to limit the number of items returned in the view (to improve render performance)

## Left to implement

Major remaining Windows Explorer parity areas:

1. **Navigation shell surfaces**
   Sidebar is currently basic user dirs/drives plus macOS-specific entries. Remaining: This PC-style device grouping, Network, removable/media volumes, trash/recycle bin browsing, sidebar tree expansion.

2. **File operation parity**
   Current copy/move/delete is strong, but Explorer has much more: pause/resume, speed/ETA details, recycle bin restore/empty, robust cross-app clipboard formats, and more exact same-volume/network behavior.

3. **Shell integration and platform associations**
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
- Spectrum analyser https://www.spek.cc/about
- PDF view: https://crates.io/crates/pdf_oxide
- EPUB: https://crates.io/crates/rbook
