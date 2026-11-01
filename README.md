# Kiln for Android

A file manager for Kiln archives. The app bundles kiln (Python 3.12 through Chaquopy) and its C
helper, so it does not need Termux, and it reads and writes the same archives.

## What it does
* Browse folders; Kiln archives (found by content, not by file name), encrypted sets and
  folders are shown, other files can be shown from the menu.
* Tap an archive to open it, enter the password if it has one, browse inside it, preview text
  and images, extract everything or just what you select, check its integrity, see its details.
* Long-press files or folders to select them, then pack them (optionally encrypted, optionally
  one archive per file with random names), rename, delete, or extract an archive.
* Long jobs show real progress, can be cancelled, and keep running with a notification when
  you leave the app.

## Build (no PC needed)
1. Push this folder to a GitHub repository.
2. Actions tab -> **Build APK** -> **Run workflow**.
3. Download the `kiln-apk` artifact, unzip it, install `app-debug.apk`.
4. Open Kiln and allow **All files access** when asked.

## Layout
* `app/src/main/python/kiln_v14.py` - your script plus one change: `KILN_ACCEL_LIB` can name a
  prebuilt C helper (an app has no C compiler). The workflow builds that helper with the NDK.
* `app/src/main/python/kiln_app.py` - everything the UI needs as plain functions returning JSON
  (scan, open, extract, pack, test, preview). Passwords are passed in memory, never through an
  environment variable or command line.
* `app/src/main/java/app/kiln/` - the Android UI (`MainActivity`), the foreground service for
  long jobs (`OpService`) and small helpers.

## Notes
* Needs Android 11 or newer. Only shared storage is visible, not Termux's private home.
* Pack runs on one core: worker processes are not safe inside an Android app.
* Each unlock re-derives the key (a few seconds on a phone), exactly like the command line.

## Version 3.0 (build 1 of 2)

Dialogs are centered; "Edit in Kiln" is offered for any small text-like file.

- New flat look that follows the system light/dark setting; no animations; bottom-sheet dialogs.
- Sort by name, size, date or type (ascending/descending, remembered). Sizes on every row.
- Works on any file: copy, move (with Replace / Skip / Keep both), delete, rename, new folder / text file.
- Tapping a file offers "View in Kiln", "Edit in Kiln", or Android's "Open with / Edit with" chooser.
- Kiln's text viewer/editor: search, wrap, text size, Save / Discard / Keep editing.
- Random-name sets can be browsed as a plain folder (menu: "Browse as plain folder").
- Needs `androidx.core` (FileProvider) - already in app/build.gradle.

Build 2 will add editing inside archives, Settings, and custom/fallback scripts.

## 3.1 (build 1 of 2)

Animations (screen transitions, rows, bottom bar, dialogs), touch ripples, ".." row, pull to
refresh, icons and previews (pictures, album art, video frames) in lists and the folder chooser,
a floating bottom bar, dialogs that follow the keyboard, new folder while copying/moving,
"Open with" with default apps, Settings, opening .kln files from other apps, and the integrity-check
fix for unsafe-path errors when the working directory is "/". Build 2 adds editing inside archives
and custom/fallback scripts.

## Version 3.2

- Modals: the dim layer now covers the whole screen (it used to be inset, which looked like a black border).
- The list no longer leaves a big gap at the bottom; the "+ New" button slides away while you scroll down and returns when you scroll up.
- Search (magnifier in the header): filters the list as you type, and also searches subfolders (can be switched off with the "Subfolders" chip or in Settings). Inside an open archive it searches the whole archive.
- Bookmarks: tap the folder name in the header for a drop-down with Internal storage and your saved folders.
- Animation speed in Settings (Off / Fast / Normal / Relaxed); the default is a little quicker than before. Pull-down and Refresh replay the row animation.
- "Wrong password" is now "Incorrect password".
