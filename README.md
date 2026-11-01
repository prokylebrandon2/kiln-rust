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
