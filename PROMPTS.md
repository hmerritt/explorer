## 1

- Research what Winscp does, how it works. Take another stab at implementing it within explorer
    - Change the sidebar icon of the remote items to "assets/icons/devices/drives/network.png" and "assets/icons/devices/drives/networkdelete.png" when not connected
    - Tab title for remote items are not correct. "music-seed" turns into "music%2Dseed". Tab title should be the original chars
    - De-bounce the UI updates for server transfers, currently it updates too often and is hard to read. limit to 500ms
    - Download speeds are currently slower than using WinSCP directly, why might this be?
- Windows taskbar icon. On window close keep running, context menu to: show version, check for updates, open new window. re-attach to an existing window
- Windows installer, see aura for an existing implmentation: https://github.com/hmerritt/aura. Squirrel installer and auto-updater

## 2

- Reorder sidebar items (in Drives, Network, WSL, etc...). Save order to settings in an object sidebar.order, where the keys are the group, and the value is an array for the order. Rename sidebar.items to sidebar.pinned.

## 3

- Settings UI
    - context-menu
        - Detect installed programs, suggest adding into menu
- Shell-extension system
- Fix Chrome file drop not registering
- UI refinement and improvements (tighten everything up, make it look nice)
- Refactor the conflict dialog for copy to include rsync-like settings (delete/keep differences, etc...)
- (maybe?) Implement a new settings item "search_recursive_max_items" for recursive search to limit the number of items returned in the view (to improve render performance)

## Left to implement

Major remaining Windows Explorer parity areas:

- GUI Settings / Preferences
  The app already has a lot of power in JSON settings: view mode, hidden files, extensions, sidebar pins, WSL visibility, columns, native icons, context menu commands. A real settings window would make existing functionality discoverable immediately. This is probably the best 80/20 feature.

- First-Class Recycle Bin / Trash
  Delete-to-trash and some undo behavior exist, but users need a browsable Trash/Recycle Bin location with restore, empty, and permanent delete workflows. This strongly improves trust around destructive actions.

- File Operation Polish
  The copy engine is already strong, including resumable copy and cancellation. The missing 80/20 layer is UX: queue multiple operations, pause/resume, ETA, clearer source/destination details, and richer conflict handling than global Replace/Skip.

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
