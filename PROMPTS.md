## New

Utility bar underneeth navigation bar. Same as Windows Explorer, this has:

- New
- Cut
- Copy
- Paste
- Rename
- Delete
- View (dropdown)
    - Show hidden files
    - File Name extensions
- Select All
- Select None
- Invert Selection

## Specific / for-later

Left side-bar drag re-sizable

---

Drag-and-drop — How should Alt-drag shortcut behavior be handled in this task?

Alt-drag should create a shortcut or simlink of the selected file/directory

---

When navigating to a directory, load in a non-blocking way. Keep the view the same initially, but if after 100ms it is still loading, preload the UI in the directory and display a loading spinner until completion. Since this is happening in a non-blocking thread, a user can choose to navigate away, in which case when the thread returns the result is simpily discarded

---

Previously — Native outbound OS dragging is explicitly staged later, per the final scope choice, because GPUI 0.2.2 does not expose the needed cross-platform drag-source/effect APIs without a GPUI patch.

Drag-and-drop — Should the plan include a local GPUI patch/vendor to expose native file drag/drop effects?

Vendor GPUI for native OS behaviour

## Didn't work first try

Signed app bundle for macOS

## Ideas

- Split-screen (Zed style)
- Shell-extension system
- 7zip built-in
- rsync copy/sync builtin
- Network drives (rclone builtin)
