## 1

- Tweak to the keybindings for Large Icons view. Large Icons view has it's own bindings that are separate to Details view. I want to tweak the behaviour slightly to improve usability. Currently on first navigation there is no selected item, and any arrow key will select the first item. I want to change this so that when no item is selected, the left arrow key can be used to go up a directory (same as Details view). The other arrow keys can still select the first item. And once there is a selection, the left arrow key should resume being a selection arrow key, to select the previous item in Large Icons view (this new behaviour only takes hold when no items are selected).
- Large icons view keybindings tweak. Currently the up/down arrow will select the item in the row directly above/below it. There is an edge case, where there IS a row below, but not enough items for there to be an item directly below the current one. Currently the down arrow will do nothing in this case. I want to make it so it will select the last item in the row below if there is not enough for there to be an item directly below.
- If clipboard content is one word (no spaces), do NOT paste into a file, do nothing with it (do not show clipboard summary UI in the status bar, or allow pasting it as a file). Only paste when content has at least one space. This is a security measure to prevent passwords being pasted as files.
- Archive file native navigation (same as Windows Explorer). Archives can be opened and navagated into like a folder.
- Status bar clipboard summary. Hide "Text file" items, since this appears anytime anything is in the clipboard. Keep functionality, but hide the UI status bar clipboard summary.
- Bug: When a file/folder is selected, and I navigate onto a different program/window, when I come back it is still highlighted (which is good) but I can't press F2 to rename without clicking on it again, or paste into the current directory.
- URL download X button (right-aligned) to cancel+delete immediately
- Paste a YouTube URL (either youtube.com or the shortened URL youtu.be) downloads via yt-dlp if it exists in the path (show error if not in the path)
- Add FTP/SFTP URL scheme for paste-to-download. Try SSH keys for seemless experience. Fall back on a username/password popup failing that.

## 2

- Fix Chrome file drop not registering
- Improve linux install.sh script (see aura's install script)
- Bug when an item (or multiple) is selected explorer sometimes freezes and enters a "not responding" state in Windows for a second or two before coming back
- Image thumbnail generation is not as fast as it used to be. TIFs take a long long time to generate (especially larger ones). All image thumbnail generation flows and pipelines need benchmarking and aggressively refactoring to improve speed (aim to reduce code in this area too as I think it is bloated).
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
