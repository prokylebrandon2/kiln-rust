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

Not yet in this slice: the progress sheet and Cancel while deleting (the spec opens it only after
250 ms), `delete_mode` (ask / recycle bin / permanent: today it is always permanent, after the
confirm), and the read-only-folder rule that comes with Root and Shizuku.

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

Not yet in this slice: the progress sheet and Cancel while copying or moving (the transfer runs on
a worker thread and cannot be cancelled), the read-only-folder rule ("Can't paste here") that
comes with Root and Shizuku, and a paste bar that follows the person to the Files tab: it shows
only on the folder screen.

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

## Not in the new screens yet (each shows a "not in the new screens yet" toast or does nothing)

Pack (the last tile of the selection bar), the progress sheet, the recycle bin, the row menus and rename, other sheets and
dialogs, thumbnails, archives (a `.kiln` tap), Recent, Starred, Settings, bookmarks, USB / SD
volumes, the crash report sheet, files shared into Kiln, the recycle bin, properties,
extract / pack.

## Next slices

1. **The row menu and rename** (the name dialog is there now), and the progress sheet with Cancel
   (shared with Delete, Copy and Move).
2. **Thumbnails** off the UI thread, keyed by path + mtime + size, inserted with `row_changed`.
3. **Recent, Starred, Settings** (placeholders in `shell.slint`).
4. **Archives, extract, check, info**; start the engine again.
5. **Delete `ui/` and the unbuilt old modules** once the parity list in `docs/UI-SPEC.md` is covered.

## Open items

- `docs/design/home-dark.png` is the mockup the dark tokens were measured from. It is not in
  the repo yet; copy it there.
- The light palette is proposed, not measured.
- `cargo fmt -p kiln-state --check` is `continue-on-error` until someone has run `cargo fmt`
  once with a compiler; then remove that line from `ci.yml`.
- `Cargo.lock`: the hand-written entries for the two new crates were accepted as they were;
  only the `kiln-app` dependency line changed. The new CI jobs still run without `--locked`;
  that can go.
- `clippy --all-targets` on the newest toolchain (1.97) flags `vec![..]` in a test in
  `app/src/thumbs.rs` (`useless_vec`); old code, not touched.
- The preview window and `slint-viewer` do not show the permission screen or the toast.
  Slice 2 added `PreviewSelect`, `PreviewSearch` and `PreviewSheet` for the new states.
