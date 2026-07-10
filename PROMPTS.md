## 1

- macOS fixes and tweaks:
    - Image viewer does not open when opening an image file with Explorer on macOS (nothing happens. Windows works correctly)
    - Remove "macOS" sidebar group. Instead add Applications and Bin to Pinned group by default (user can then unpin/remove as usual)
    - (compression)
- Improve readme - add screenshots / branding

## 2

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

- GUI Settings / Preferences
  The app already has a lot of power in JSON settings: view mode, hidden files, extensions, sidebar pins, WSL visibility, columns, native icons, context menu commands. A real settings window would make existing functionality discoverable immediately. This is probably the best 80/20 feature.

- Explorer-style Shell Sidebar
  Add first-class places like This PC, Network, Recycle Bin/Trash, removable drives, drive capacity display, and expandable sidebar folders. The current sidebar has pinned folders, drives, WSL, and macOS locations, but not the full Windows Explorer shell model.

- Cross-App File Clipboard
  Copy/cut/paste currently appears mostly app-private. Supporting native file clipboard formats would let users copy from Finder/Nautilus/Dolphin/Windows Explorer into this app and vice versa. This is a huge “feels real” improvement.

- First-Class Recycle Bin / Trash
  Delete-to-trash and some undo behavior exist, but users need a browsable Trash/Recycle Bin location with restore, empty, and permanent delete workflows. This strongly improves trust around destructive actions.

- File Operation Polish
  The copy engine is already strong, including resumable copy and cancellation. The missing 80/20 layer is UX: queue multiple operations, pause/resume, ETA, clearer source/destination details, and richer conflict handling than global Replace/Skip.

- Preview / Details Pane
  Alt-hover previews and rich Properties are already implemented. A right-side Preview/Details pane would make browsing much faster, especially for images, video, audio, text/code, PDFs, and metadata-heavy files.

- Open With / Default App Management
  Open With support exists, but the Windows Explorer-style flow of choosing an app, setting defaults, and managing associations would close a common daily-use gap.

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
