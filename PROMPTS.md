## 1

- New UI feature: Hovering over the status bar clipboard summary button shows a popup (large-tooltip esk) UI box with more information about the content in the clipboard and the action that will be taken.
    - Folder count, file count, URL address, RAW text content, and total size

## 2

- Image thumbnail generation is not as fast as it used to be. TIFs take a long long time to generate (especially larger ones). All image thumbnail generation flows and pipelines need benchmarking and aggressively refactoring to improve speed (aim to reduce code in this area too as I think it is bloated).
- UI refinement and improvements (tighten everything up, make it look nice)
- Refactor the conflict dialog for copy to include rsync-like settings (delete/keep differences, etc...)

## 3

- Settings UI
    - context-menu
        - Detect installed programs, suggest adding into menu
- Split-screen (see Zed)
- Shell-extension system
- Fix Chrome file drop not registering
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
