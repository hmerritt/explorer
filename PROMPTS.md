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

---

Implement a perminent left side-bar.

Segments (separated via a separator line):

- User pinned programs (first implementation, ignore/skip this)
- User area directories (user directory, desktop)
- List all local drives (main OS drive listed first)

## Specific / for-later

When navigating to a directory, load in a non-blocking way. Keep the view the same initially, but if after 100ms it is still loading, preload the UI in the directory and display a loading spinner until completion. Since this is happening in a non-blocking thread, a user can choose to navigate away, in which case when the thread returns the result is simpily discarded

---

## Didn't work first try
