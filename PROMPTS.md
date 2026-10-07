## 1

## 2

- macOS tabs cannot be dragged into a split view, dragging a tab currently drags the entire window
- OS interop for windows currently passes file/folder paths with forward slashes. Not all programs can handle this and it can break.
- Deleting a file in Large Icons view with many files (that scrolls) will reset the scroll position. It should auto-select the previous item (or next in the case of a first item) and maintain the place

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
