## New

Mouse back/forward buttons key binding to back/forward opperation.

General key bindings:

- Up/Down arrows to move up/down
- If no file is selected:
    - Down arrow selects first file
    - Up arrow selects bottom file
- Left arrow to go back
- Right arrow to enter into the selected directory
- Enter to enter into the selected directory
- F5 to refresh
- Shift modifier. Shift+
    - Up/Down arrows select multiple files in the direction (after last file, does nothing)
    - Home key, selects all files above current selection (inclusive)
    - End key, selects all files below current selection (inclusive)
- (suggest others if you can think of any)

---

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

---

Implement a perminent left side-bar.

Segments (separated via a separator line):

- User pinned programs (first implementation, ignore/skip this)
- User area directories (user directory, desktop)
- List all local drives (main OS drive listed first)

## Specific / for-later

When navigating to a directory, load in a non-blocking way. Keep the view the same initially, but if after 100ms it is still loading, preload the UI in the directory and display a loading spinner until completion. Since this is happening in a non-blocking thread, a user can choose to navigate away, in which case when the thread returns the result is simpily discarded

---

Improve repo src/ code organisation. Split out explorer.rs into each dinstinct part.

Let's plan out together which things to split, and how

---

## Didn't work first try

Explorer columns add ellipsis when text clips the column

---
