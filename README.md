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

## 3.11 (second half, checkpoint 1): editing inside archives

- Open an archive and use the bottom bar: **Add** (files or folders from the phone, a new folder, or a new text file), and on selected items **Copy, Move, Delete**; menu has **Rename**.
- Edit a file's text from inside an archive ("Edit as text in Kiln"), or "Edit with..." in another app (asks to update the archive when you return).
- Nothing is written until **Save**. A bar shows "Unsaved changes" with Save / Discard, and leaving with changes asks Save / Discard / Keep editing.
- Works for plain archives, encrypted archives and random-name sets (kiln's own staged-edit engine; saving swaps through a temporary file).
- Extract and Check are blocked while there are unsaved changes (they read the saved archive).
- Still to come (checkpoint 2): Settings for main/fallback scripts and Tools, flash drives, Termux home.

## 3.14

- Main and fallback scripts now have a signature: the first bytes of the files they handle. Choosing a
  script reads its `MAGIC` constant; if it has none you type one (text like `KLNA` or hex like
  `0x4B4C4E41`). It can be changed later in Settings. The file list uses the main script's signature
  first, then the fallback's. Opening is still main first, then fallback.
- A fallback script now takes effect straight away (no restart needed).
- The "Termux home" bookmark is gone.
- Back remembers where you were in a folder, including folders inside an archive.
- Archive info and the Benchmark and WAV info results have proper layouts.

## 3.13

- The file menu inside an archive now has an icon on every row, with dividers between groups.
- Self-test, Benchmark and WAV info show progress (the self-test has a bar that counts its checks)
  and can be cancelled.
- A tidier About box: mark, version, and a details card (engine, fast mode, scripts, encryption).

## 3.12 (second half, checkpoint 2)

- **Settings > Scripts:** a custom *main script* (replaces built-in v14 at the next start; checked when chosen and falls back to v14 if it fails to load; v14 can always be switched back to). It has a signature, the first bytes of the files it handles.
- **Settings > Tools:** Self-test, Benchmark and WAV info, with the output shown in a results box.
- **Settings > Advanced:** parallel jobs for packing (1 default; 2 and 4 are experimental).
- **Bookmarks:** mounted flash drives / SD cards are listed while plugged in (they disappear when unplugged); "Termux home" is there too, with an explanation when Android blocks it.


## 3.15

- The fallback script feature is removed. Any fallback script saved by an earlier version is deleted at start-up. Custom main scripts still work, with their signature.
- Back in a folder now scrolls once and slides all visible rows in (the second scroll made lower rows skip the animation).
- Check (inside an archive) shows a result card: verdict, files, data and time tiles, and details.

## 3.16

- Folder listing now runs in Java: entries are read and checked for the archive signature natively, several at a time in big folders (64+ entries). If anything goes wrong it falls back to the Python scan.
- Folder item counts ("12 items") load right after the list appears instead of delaying it. Counts seen before show immediately.

## 3.17

- **Opening a big file from an archive no longer fills the cache twice.** A file you open is extracted once into the preview cache and found there on later opens (also after you restart Kiln). The cache is keyed by archive, its size and date, and the file, so a changed archive gets a fresh copy.
- The preview cache is built under a temporary name and renamed when complete, so a cancel or a crash never leaves a half-extracted file that looks finished. It is kept under 1 GB (least recently used removed first, nothing older than a week). Settings > Storage shows its size and can clear it.
- Opening a file checks the free storage first and says so if the file won't fit.
- **Cancel everywhere it can matter:** opening a file from an archive, "Edit with...", preparing an archive for editing, and copying in a file opened from another app. Extraction, packing, check, save, copy, move and delete already had Cancel.
- **Cleanup on cancel or error:** a half-written extracted file is deleted (before, it stayed on disk). A cancelled extraction removes the files and folders it created. A cancelled copy undoes itself. A cancelled or failed copy of a staged file leaves nothing behind.
- Viewing an image from an archive no longer deletes its cached copy when you close the viewer.
- A file opened from another app (a private copy in the cache) is deleted when you close the archive, and an earlier one is removed when you open the next.
- Scratch folders from editing sessions that were cut short are cleaned up.

## 3.18

- **Percentages on every progress box:** the file name on the left, the percentage on the right, the bar underneath (also in Settings > Tools and when a file is copied in from another app). A "Keep the screen from turning off" tick is there for long jobs.
- **Properties** (select something, then the menu): type, size (with the exact byte count), last modification, permissions, file system path and type. Files also show an MD5 (calculated in the background). Folders show the file count and total size as they are counted, and a "Disable media scanning" tick (adds or removes `.nomedia`). Kiln archives add archive properties: type, encryption, file count, content size and compression ratio. Several items show combined totals. Inside an archive it shows what the archive's table knows about the selected entries. Closing the box stops any counting or MD5 still running.

## 3.19

- The Pack result is a card like Check's: a header (archive, lock or set icon), tiles for how many files or archives were made, the stored size and the time, a bar showing how much space was saved, and a details card (location, layout, encryption).
- Pressing Back in the home folder no longer leaves Kiln at once: it shows "Tap Back again to exit", and a second press within two seconds exits.

## 3.20

- **Pull-down info:** dragging the list down at the top reveals type, free/total space, folder name, folder size (counted live) and a usage bar. Pull-to-refresh still runs on release. In an open archive it shows the archive's name, size and file count.
- **Smoother progress that names the current file:** the box shows the file being packed or extracted right now. The bar eases toward each report and keeps moving at the measured speed between reports. "One archive per file" follows total bytes instead of jumping per file. The notification updates at most twice a second.
- **Flash drives and SD cards are storages of their own:** a drive's top folder acts as a home (no ".." row; Back shows "Tap Back again to exit"). The title shows the drive's name. Kiln still opens to Internal storage. The folder chooser lists the other storages at a storage's top. If a drive is unplugged while you are in it, Kiln goes home.
- **Delete source files after packing:** Settings > Packing (off by default) and a tick box in the Pack dialog. Only files confirmed inside a finished archive are removed, and only when the whole job succeeds; skipped files stay, and empty folders are tidied. The result card shows how many were deleted.
- **Bookmarks show space:** storages show free/total; bookmarked folders show how much they hold (counted in the background).
- Pack result no longer says "random names" for unencrypted one-archive-per-file packs.

## 3.21

- **Pull-down strip no longer clips the list:** the "can't open..." / empty-folder message used to stay put while the list slid down, so it landed on top of the ".." row. The message now moves with the list, and slides back with it on release.
- **Search results show where they are:** every hit has its own line with the folder it is in (archives, sets, folders and files alike, including hits directly in the searched folder). Settings > "Path shown in results" still picks relative or absolute; long paths are cut at the start so the deepest folder stays visible.
- **New "Extraction complete" screen:** header with the archive name, tiles for files, size and time, and a card with where it was saved, the source archive, the checksum result and whether files were replaced. Buttons: OK and Open folder.
- **Self-test result redone:** a verdict header, tiles for checks / passed / time, failures listed first in red with their message, then every passed check with its time. The raw text list is still the fallback if the output isn't in the usual shape.
- **Files without an extension:** Kiln looks at the first 8 KB when a name doesn't say what a file is. Text opens in Kiln's viewer and editor (empty files too); an extensionless picture opens in the image viewer; binary data no longer gets a "View as text" option (from inside an archive, Kiln says it looks binary and offers Open with...). Other apps are told the real type (text/plain, image/png, application/pdf, application/zip, ...), so "Open with" lists the right apps. Properties shows the detected type.

## 3.22 (checkpoint 1 of 2: icons)

- **Distinct file icons, with colour:** PDF, documents, spreadsheets, slides, apps (APK), archives (zip, 7z, rar, tar...), fonts, databases, web pages, shell scripts, keys and certificates, e-books, disk images, executables, plain text, config files and code each have their own shape. Colours are muted so they work in light and dark. Pictures, music and video keep their previews.
- **Standard Android folders have their own icons:** Download, DCIM (and Camera), Pictures (and Screenshots), Movies, Music (also Audiobooks, Podcasts, Recordings), Documents, Android, and Alarms / Notifications / Ringtones. They match only at the top of a storage, so your own folder called "Music" elsewhere stays plain.
- The folder chooser uses the same icons.
- Groundwork for checkpoint 2: `Icons.java` holds the list of folder icons (folder, download, camera, pictures, movies, music, documents, android, sounds, archives, favourite, heart, private, code, home, phone, SD card, USB), ready for choosing one per folder and per bookmark.

## 3.24
- Folder icons re-centred; music icon redrawn (generator: `tools/gen_folder_icons.py`, needs Pillow + cairosvg).
- Pick an icon for any folder (select it, menu > "Change icon...") or for a bookmark (tap its icon, or long-press the row).
  Choices are kept per path and follow the folder when it is renamed in Kiln. "Use default" restores the standard look.
- Bookmarks now show their folder's icon.

## 3.25
- Bookmarks no longer work out how much each folder holds (no background scan, no size label).
