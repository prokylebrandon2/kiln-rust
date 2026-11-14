# UI rewrite

The Slint UI in `ui/` is being replaced by a new one in `ui-next/`, on top of a UI-independent
`state/` crate. This file is the plan and the status.

Status: slice 1 is in. The app now starts `ui_next::MainWindow` (the old window is no longer
built), and the Files tab, folder browsing, the Back key, the permission screen and toasts work.
Everything else from the old UI is missing until its slice (list below). This was compiled and
run with a compiler for the first time in slice 1: `cargo test -p kiln-state`, `cargo clippy -D
warnings` and `cargo fmt --check` on `kiln-app`, `kiln-state` and `kiln-ui-next` are clean, and
`--locked` accepts `Cargo.lock`. The app crate was also run headless on Linux against a fake
phone (screenshots of the Files tab, a 400-file folder, scrolling, Back with the scroll
position restored); it has not been run on a phone.

## Why

What the code says today (counted, not guessed):

- `ui/ui/` is 5,528 lines of Slint. `main.slint` alone is 2,218 lines: one `MainWindow` with
  224 properties and 69 callbacks, so any screen change touches one huge component.
- `render_rows` (`app/src/browse_ui.rs`) rebuilds the whole row model with `set_vec` whenever a
  folder, sort or search changes. For every row it also looks up thumbnails and the folder
  icon / colour maps, on the UI thread.
- Each row is a `KListRow` with its own `PressArea`, which has two `Timer`s and a ripple. The
  whole Slint tree has 34 `animate`s, 6 `Timer`s, 7 drop shadows and 11 `clip: true`.

So: laggy because rows are rebuilt and measured in bulk, messy because everything hangs off one
window component.

## Shape of the new UI

1. `state/` (`kiln-state`): no Slint, no Android, no engine. `Entry`, `Listing` (sorted and
   filtered view plus a selection that survives re-sorting), natural-order sorting, size and
   date formatting that mirrors `app/src/fmt.rs`, and a two-phase scan: names and kinds first,
   sizes / dates / item counts after, for the rows in view first. Unit tests live next to the code.
2. `ui-next/` (`kiln-ui-next`): `tokens.slint` (every colour, size and type size),
   `controls.slint`, one file per screen, and `state.slint` with one global per screen
   (`Nav`, `HomeState`, `BrowseState`) as the only interface between Rust and a screen.
   Lists are `FileList` (`list.slint`) on the stock `ListView` (the only `std-widgets` import):
   rows are all `Tok.row-h` high and `ListView` builds only the ones in view. Three things about
   it matter. A `ListView` takes one `for` and nothing else, so the room under the last row
   (`bottom-inset`) is made of blank rows. The default style (Fluent) embeds SVG images and rustc
   fails on the generated code (`load_image_from_embedded_data`, no decoder in this build), so
   `ui-next/build.rs` selects Cupertino explicitly. And the style's scroll view pans by touch only
   and draws a scrollbar, so `mouse-drag-pan-enabled` is on and the scrollbars are off.
   (The first version of the screen used `ListView`, then a hand-built list of rotating row slots
   replaced it, then `ListView` came back with the style fixed.)
   `preview.slint` fills the globals with sample data for `slint-viewer`.
3. Rust glue (`app/src/screens.rs`): `ListingModel` is a Slint `Model` over a `Listing` that
   builds an `EntryVm` only when the list asks for a row, so opening a folder of ten thousand
   files costs one `read_dir`. Sizes, dates and item counts are fetched on worker threads for the
   rows in view and forwarded as `row_changed` (a sort by size or date fetches all first).

Rules kept from the old code: edition 2024, `unsafe` only in `platform/`, no new dependencies
(`ui-next` uses the same pinned `slint` / `slint-build`), no `std-widgets` except `ListView` (`tools/ui/slint_lint.py` fails on any other).

## Done (slice 0)

- `state/`: entry, fmt, order, listing, scan, with tests.
- `ui-next/`: tokens, controls, rows, nav, Files tab (home), folder screen, shell, preview.
- `tools/ui/slint_lint.py` (static check, not a compiler) and `tools/ui/gen_icons_next.py`
  (generates `ui-next/ui/icons.slint` from the legacy icon paths).
- CI jobs `state` (fmt non-blocking, clippy, test) and `ui-next` (lint, icon freshness,
  `cargo check`), both without the Android SDK.

## Done (slice 1)

- `app/src/lib.rs` is the new, small app; `app/src/screens.rs` is the glue. `app` depends on
  `kiln-state` and `kiln-ui-next` instead of `kiln-ui` (two lines of `Cargo.lock`).
- Files tab: storage card (`Host::space`), folders of the storage with item counts and dates,
  quick-access tiles that open Download, Pictures, Movies, Music, Documents.
- Folder screen: virtual list, folders first, natural sort and the saved sort / hidden / show-all
  settings, "Loading..." after 700 ms, "This folder is empty.", errors as text, the scroll
  position restored when going back, a file opens through the system "Open with" list.
- Back key and back button: up a folder, out of the folders, to the Files tab, out of the app.
  Safe-area insets, the permission screen ("All files access") and a toast.
- The old window's modules (`about_ui`, `arc_ui`, `browse_ui`, `dialog`, `extract_ui`,
  `folder_ui`, `marks`, `open_ui`, `pack_ui`, `props_ui`, `settings_ui`, `thumbs_ui`, `pull`)
  stay on disk but are not declared in `lib.rs`: they are the reference for porting. `io_text`
  moved from `browse_ui` to `fmt`. The engine is not started yet (nothing uses it until
  archives).
- Row icons are neutral, as in the mockup (the old per-type colours are not used).

## Done (slice 2)

- Selection: a long press selects a row, a tap toggles it while anything is selected, the bar
  shows "N selected" and its left button (the cross) clears the selection. Select all and Clear
  selection are in the menu.
- Search: the magnifier opens a field under the bar (it takes the keyboard focus), every word
  must occur in a name, "No matches." when nothing does; the cross closes it and clears it.
- Sort: the sort button opens the "Sort by" sheet (Name, Size, Date modified, Type, then
  Ascending / Descending); a choice applies at once and is kept in `sort_key` / `sort_asc`.
- The menu button: Select all, then (with nothing selected) Show archives only / Show all files,
  Show hidden files / Hide hidden files, Refresh. Refresh reads the folder again and keeps the
  selection (by name), the search and the place in the list.
- One modal `Sheet` (`ui-next/ui/sheet.slint`, `SheetState`): a card over a scrim, lines with an
  optional icon and a check; Rust fills it and says what each line does (`SheetAction`).
- Back key: closes the sheet, then leaves the selection, then the search, then goes up a folder.
- `Listing` gained `selected_names` / `select_names` (a selection that outlives a re-read),
  `has_query` and `shows_all`.

## Done (slice 3a)

- The selection bar (`BrowseScreen`, `ui-next/ui/browse.slint`): Copy, Move, Delete (danger tint)
  and Pack tiles on a `k_surface` card, 12 dp from the sides and the bottom, floating over the
  list. The list has `bottom-inset` so the last row can be scrolled clear of it. Differences from
  the spec: the bar appears and goes at once (the 240 ms slide and fade are not built).
- `Dialog` (`ui-next/ui/dialog.slint`, `DialogState`): title, text, Cancel (text) and a confirm
  pill, in the danger colour when it deletes; a tap outside is Cancel; Back closes it before the
  sheet. With no Cancel label it is a message with one OK button (`App::message`).
- Delete: "Delete <what>?" (the quoted name for one item, "N items" for more), "This can't be
  undone.", Cancel / Delete. It runs `fs_delete` on a worker thread (`kiln_bridge::call`), a
  failure opens the message "Couldn't delete everything" with the engine's text, and the folder is
  read again (the selection is gone). Only rows that are selected **and in view** are deleted: a
  row that a search hides is never touched, even though the bar still counts it.
- Pack still toasts "... is not in the new screens yet." (Copy and Move are slice 3b.)
- `kiln_state::fmt::what` (tested) builds the "<what>" of a question about the selection.
- Icons `copy`, `move` and `delete` were added to `gen_icons_next.py`.

Not yet in this slice: `delete_mode` (ask / recycle bin / permanent: today it is always
permanent, after the confirm), and the read-only-folder rule that comes with Root and Shizuku.
(The progress sheet and Cancel came with slice 4b.)

## Done (slice 3b)

- Copy and Move (the first two tiles of the selection bar) hold the selected rows that are in
  view, let go of the selection, and toast `Open a folder, then tap "Paste here"` (`"Move here"`).
- The paste bar (`PasteBar`, `ui-next/ui/browse.slint`, `BrowseState.paste-*`): while something is
  held and nothing is selected it floats over the bottom of the list like the selection bar:
  "Copying N items" or "Moving N items" (15 sp bold), "Into <folder>" (12 sp), then New folder,
  Cancel and Paste here / Move here. The folder is "Internal storage" at the top of the storage.
  The list has `bottom-inset` for it. New folder opens the name dialog (slice 3c).
- Paste here runs `fs_plan` on a worker thread. If it fails: "Can't copy" or "Can't move" with the
  engine's reason. With nothing in the way it runs `fs_transfer` (policy replace); otherwise the
  sheet "1 item already exists" / "N items already exist" opens: the first five names as
  "• name" lines and "and N more", then Replace, Skip existing and Keep both (no icon) and a text
  Cancel (the items stay held). `SheetState` gained `body-lines` and `cancel-label` for it.
- When the transfer is done the items are let go of, the engine's message is toasted ("Cancelled",
  or the sheet "Not everything was copied" / "moved" on failure) and the folder is read again.
- `kiln_state::fmt::clashes` (tested) builds the title and the lines of the "already exists" sheet.

Not yet in this slice: the read-only-folder rule ("Can't paste here") that comes with Root and
Shizuku, and a paste bar that follows the person to the Files tab: it shows only on the folder
screen. (The progress sheet and Cancel came with slice 4b.)

## Done (slice 3c)

- "+ New" (`NewButton`, `ui-next/ui/browse.slint`): a 48 px pill with a plus, 16 px from the bottom
  right. It goes while the list scrolls down by more than 6 px at a time, comes back on the way up,
  and is always there at the top; it is hidden while rows are selected or the paste bar shows. The
  list keeps 80 px free under its last row for it.
- Tapping it opens the "New" sheet (Folder, Text file, Cancel text), which opens the name dialog.
- The dialog has a text field now (`DialogState.field`, `field-text`, `error`): "Name" hint, the
  text starts selected (`untitled.txt` for a text file, empty for a folder), Enter confirms, and a
  name that can't be used keeps the dialog open with a red line under the field and a red outline:
  "Enter a name (no slashes)" (empty, a slash, `.` or `..`; `kiln_state::name::check`, tested),
  "That name is taken", or the system's reason. Typing clears the line. Back closes the dialog.
- New folder in the paste bar opens the same dialog; the held items stay held.
- A made folder or file is created on the UI thread (it is one call) and the folder is read again.
- Icons `new-folder` and `new-file` were added to `gen_icons_next.py`.

Differences from the Java: the Java trims the name and refuses only empty and slashes; `.` and `..`
are refused here too. Not yet: a new file or folder in a folder the engine must write as root
(`Priv`, with Root and Shizuku), and the row slide-in after creating.

## Done (slice 4a)

- Rename: with exactly one row selected (and in view) the menu has a "Rename" line (pencil icon)
  after "Properties" and before "Icon and color..." and "Clear selection", the order the Java menu
  has. It opens the name
  dialog titled "Rename" with Cancel / Rename; the field starts with the current name and the part
  before the last dot selected (`report.pdf` starts with `report` selected; `.nomedia` is selected
  whole), like the Java.
- `Listing::only_selected` (tested) is the one-row rule: exactly one row is selected and it is not
  hidden by a search, so Rename never acts on a row the person can't see (the rule Delete has).
- The name goes through `kiln_state::name::check` (trimmed; not empty, `.`, `..`, with a slash).
  A name that can't be used keeps the dialog open with the red line, as in New: "Enter a name (no
  slashes)", "That name is taken" (any entry of that name, a dangling link too, so nothing is ever
  replaced), or the system's reason. Typing clears the line.
- The same name is not a change: the dialog closes and the selection stays. Otherwise the folder
  is read again (the selection is gone) and a renamed folder's icon and colour follow it
  (`Prefs::move_folder_icons`).
- `kiln_state::name::stem_end` (tested) is where the selection ends; `DialogState.select-to` carries
  it to the field (-1 selects all, as before), and `open_dialog` resets it for every dialog.
- Icon `edit` was added to `gen_icons_next.py`.

Differences from the Java: the Java says "Couldn't rename" for any failure; this says what the
system said. The Java answers the same name with "That name is taken"; this just closes. Not yet:
Rename in a read-only folder (the Java hides it; that rule comes with Root and Shizuku), and
`Priv` (Rename as root).

Slice 4a was written in a session with no Rust toolchain; slice 4b's session had one (Rust 1.100)
and compiled it: `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` pass, and the
`set-selection-offsets` call and the borrow scopes it worried about compile as written. What no
session has done is run Rename on a phone: the selection offsets in particular are unchecked on
a real keyboard.

## Done (slice 4b)

- The progress sheet (`ui-next/ui/progress.slint`, `ProgressState`, `app/src/progress.rs`),
  shared by Delete, Copy and Move. `App::run_job(title, work, done)` runs `work(&Job)` on a worker
  thread and calls `done(&App, &Value)` on the UI thread with the engine's result; it returns
  false (and does nothing) while another operation runs. Delete uses `kiln_bridge::call_job(
  "fs_delete", ..)` and titles the sheet "Deleting..."; a transfer uses `fs_transfer` and "Copying..."
  or "Moving...". `delete_done` and `transfer_done` are unchanged.
- The sheet opens only after 250 ms, so a quick operation never shows it. It is a card over a
  scrim with the title (19 sp bold), the current file (14 sp, muted, cut at the end) with the
  percent (16 sp bold, rounded down) at its right, a bar and an outline Cancel. While the amount is
  not known the percent goes and the bar slides (`ProgressBar.indeterminate`, 1.4 s a trip).
- Back and a tap outside do nothing while an operation runs (`App::on_back` returns first). Cancel
  calls `Job::cancel`, turns the file line into "Cancelling...", greys Cancel out and keeps the
  sheet up until the work returns; the usual result follows ("Cancelled" toast). A cancelled copy
  is undone by the engine, a cancelled move or delete stops where it is.
- A progress event or result that belongs to an operation that has ended is dropped (each operation
  has a number). A worker that panics outside the engine ends the sheet with a message instead of
  leaving it up for ever.
- `ui-next/ui/preview.slint` has `PreviewProgress` (`slint-viewer --component PreviewProgress`).
- Tests (`app/src/progress.rs`): the event reading and the percent are pure and tested; a delete
  and a copy are run through the real engine with a job whose progress callback cancels it, and
  the tests check that the delete stops part way and that the copy leaves nothing behind.
  The sheet was also rendered headless with Slint's software renderer to check the layout
  (progress and cancelling states); that harness is not in the repo.

Differences from the Java (as in docs/UI-SPEC.md): no "Keep the screen from turning off" box, no
notification or foreground service after 1.5 s, the bar jumps to each report instead of easing.
Not yet: the recycle bin jobs ("Moving to recycle bin...", "Emptying recycle bin..."), Pack, Extract
and Check, which will use `run_job` when they come; and "Cancelled after N items" (the engine's
cancel result carries no count for Delete or Copy, so the toast says "Cancelled").

## Done (slice 4c)

- Properties: while rows are selected the menu has a "Properties" line (the `file` icon, as in the
  spec) right after "Select all". It opens the sheet for the rows that are selected **and in
  view** (`ui-next/ui/props.slint`, `PropsState`, `PropRow`; `app/src/props_sheet.rs`). The card
  scrolls its lines; Copy puts them on the clipboard as text ("Copied" toast) and keeps the sheet
  up; OK, Back and a tap outside close it.
- One item: its name, "General information" with Type, File count (folders), Size, Last
  modification, Permissions, FS path and FS type (when the mount is known) and MD5 (files). An
  archive adds "Archive properties" (Type, Encrypted, File count, Content size, Compression
  ratio). Several items: Items, Location, File count, Size, Folders. A folder also shows the
  "Disable media scanning" box, which makes or removes `.nomedia` (and goes back to what the
  folder really has if that fails).
- The slow lines say "Calculating..." and are filled in from worker threads (the walk, the MD5,
  the archive probe) with `slint::invoke_from_event_loop`; closing the sheet (or opening another)
  sets a stop flag and later answers are dropped.
- What the sheet shows first is worked out by `plan_one` / `plan_several`, which use no window and
  no thread and are tested (`app/src/props_sheet.rs`). The data comes from `props.rs`, which was
  already built. `DialogButton` is exported from `dialog.slint` so the sheet can use it.
- `PreviewProps` (`slint-viewer --component PreviewProps`); the layout was also rendered headless.

Differences from the Java: as in docs/UI-SPEC.md part 6 (values are plain text, no 300 ms wait),
and a set shows a folder's lines. Not yet: the properties of what is selected inside an archive
(waits for the archive part), and the threads and the Copy button have not been tried on a phone.

## Done (slice 4d)

- Icon and color...: with exactly one folder selected (a plain folder, not a set or an archive)
  the menu has this line (the `folder` icon) after "Rename". It clears the selection and opens the
  picker (`ui-next/ui/pick.slint`, `PickState`, `PickCell`; `app/src/pick_sheet.rs`): 11 swatches
  ("default" in the accent, then the 10 colours of the spec) and the 18 folder icons, 4 to a row.
  A swatch saves its colour, moves the tick, re-tints the icons and the row behind, and the sheet
  stays open. An icon saves its key and closes. "Use default" clears the icon and the colour.
  Cancel, Back and a tap outside change nothing. `open_pick(path, name, with_colors)` also does
  the picker without swatches (the bookmark's), which nothing opens yet.
- The 17 folder glyphs the picker needs are in `Icons` as `dir-download`, `dir-camera`, ... (from
  the legacy set by `tools/ui/gen_icons_next.py`; `folder` is the plain one).
- Rows now show what was picked. `Looks` keeps a copy of the picked icons and colours (`Marks`,
  loaded at start and after every change, so drawing a row never parses a setting) and
  `ListingModel` knows its folder. A folder row gets: the icon picked for it, else the standard
  icon of Android's own folders (Download, DCIM, Music... as in the Java's `dirIcon`), else the
  plain folder; and the colour picked for it, else the default folder colour (the setting, which
  has no screen yet), else the theme's. Only `Kind::Dir` gets this; a set keeps the plain folder
  (the spec says an encrypted set must not, and a plain set cannot be told from one here).
  The Files tab rows follow too. After a change only the one row is redrawn (`row_changed`), and
  the Files tab is read again when the folder sits in the top of the storage. A rename moves the
  marks and reloads the copy.
- `PreviewPick` (`slint-viewer --component PreviewPick`); the sheet was also rendered headless.
  `DialogButton` has a new `outlined` look for "Use default".

Not done: the picker for a bookmark's icon (the bookmarks drop-down is not built), and the sheet
has not been tried on a phone.

### Slice 4e: opening an archive (read only)

- A tap on a `.kiln` row reads its contents behind the progress sheet ("Opening...", the engine's
  `open_archive`). A locked one asks for the password in the dialog ("Unlock <name>", the field in
  password mode): a wrong password toasts "Incorrect password" and asks again, another failure is
  the dialog "Couldn't open <name>". A set (an encrypted folder) asks for its password first;
  "Browse as plain folder" in the selection menu (one set selected) opens it as an ordinary folder.
- The rows are the folders and files directly in the folder shown (`ArcState::list_entries`), as
  entries of an ordinary `Listing`, so sort, search and selection work unchanged. A folder shows the
  number of files below it. A folder is entered with a tap; Back goes up, to where that folder was
  scrolled, and at the top leaves the archive for the folder it was opened from. `Screens::arc`
  holds the open archive, and the folder it came from stays the last of `levels`.
- Nothing can touch real files from inside: "+ New", the paste bar and the lookup of sizes and
  counts are off, Refresh is not in the menu, and the bar's tiles toast "not in the new screens
  yet". The menu has Select all and Clear selection only.
- Not done: a file's sheet, Extract, Check, Info, editing and saving, "Show password", Rename and
  Properties of archive rows, the archive's own bar. Search inside an archive filters the folder
  shown (the Java matches names in the whole archive).

## Not in the new screens yet (each shows a "not in the new screens yet" toast or does nothing)

Pack (the last tile of the selection bar), the recycle bin, the rest of the
selection menu (Open options..., Extract...), other sheets and
dialogs, thumbnails, the archive tools (Extract, Check, Info, editing), Recent, Starred, Settings, bookmarks, USB / SD
volumes, the crash report sheet, files shared into Kiln, extract / pack.

## Next slices

1. **The rest of the selection menu** (Open options..., Extract...). Rename (slice 4a), the
   progress sheet with Cancel (4b), Properties (4c) and Icon and color (4d) are done; both of
   what is left need the file sheet and the archive screens, so they belong with slice 4. Pack, the
   recycle bin and the archive operations will run behind `App::run_job`.
2. **Thumbnails** off the UI thread, keyed by path + mtime + size, inserted with `row_changed`.
3. **Recent, Starred, Settings** (placeholders in `shell.slint`).
4. **Archives, extract, check, info**; start the engine again.
5. **Delete `ui/` and the unbuilt old modules** once the parity list in `docs/UI-SPEC.md` is covered.

## Open items

- `docs/design/home-dark.png` is the mockup the dark tokens were measured from. It is not in
  the repo yet; copy it there.
- The light palette is proposed, not measured.
- Rename, the progress sheet, Properties and the icon picker have been compiled and tested, but
  not run on a phone.
- Standard icons for Android's own folders (Download, DCIM...) are back in the list with the
  picker, as in the Java. The old design mockup shows plain folders; if the new design wants
  them plain, `Looks::dir_look` is the one place to change.
- `Cargo.lock`: the hand-written entries for the two new crates were accepted as they were;
  only the `kiln-app` dependency line changed. The new CI jobs still run without `--locked`;
  that can go.
- The preview window and `slint-viewer` do not show the permission screen or the toast.
  Slice 2 added `PreviewSelect`, `PreviewSearch` and `PreviewSheet` for the new states.
