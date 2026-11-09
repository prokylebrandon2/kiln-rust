# Kiln UI specification

Source of truth for the Slint UI in `ui/ui/*.slint`. Everything here was read out of the original
Android app (`Ui.java`, `Sheet.java`, `MainActivity.java`, `res/`), not designed. When this file
and the Java disagree, the Java wins and this file is wrong. The Java stays in the tree until the
port is finished so that it can be checked (`app/src/main/java`, `res/`,
`app/src/main/AndroidManifest.java-app.xml`).

Phase A covers the design tokens, every shared component, and the **main screen before a folder is
loaded**. The remaining screens (file list, pull-down panel, bottom bar, menus, Settings, Text
editor, Image viewer, Properties, archive screens) are added to this file in the phase that builds
them, read from the Java at that time.

## Units and conventions

- **dp.** 1 Slint logical px = 1 dp. Slint's Android backend sets the scale factor to
  `densityDpi / 160`, which is `DisplayMetrics.density`. The Java rounds each dp value to whole
  pixels (`Ui.dp`); Slint does not, so edges can differ by up to half a physical pixel.
- **sp.** Text sizes are in sp: `Theme.sp(15px)` = 15 dp x the system font scale. The font scale
  (`Configuration.fontScale`) is read by the `platform` crate and set on `Theme.font-scale`.
- **Hairlines.** `dp(1)` in the Java is `Math.round(density)` physical pixels (`Theme.hair`);
  `Ui.rule()` is exactly one physical pixel (`Theme.hair-px`).
- **Colours.** Android writes `#AARRGGBB`, Slint `#RRGGBBAA`. The conversion is done once, in
  `theme.slint`; the table below shows both.
- **Font.** `Typeface.DEFAULT` / `DEFAULT_BOLD`: the system sans (Roboto). Weight 700 for bold.
  Slint resolves "Roboto" through the system fonts; where a phone has no Roboto, Slint falls back
  to its default sans, as `Typeface.DEFAULT` would fall back to the phone's own.
- **Dark mode** follows the system (`res/values-night`), live: the app declares `uiMode` in
  `configChanges` and the `platform` crate reports the change, so nothing restarts.
- **System bars.** Not edge-to-edge. The theme gives status and navigation bars `k_bg`, with dark
  icons by day and light icons by night (`windowLightStatusBar` and `windowLightNavigationBar` are
  `k_light_bars`: true by day, false by night; `KilnTheme`); this is theme XML and stays
  in `app/src/main/res/values(-night)/styles.xml`. The window starts below the bars, so the
  safe-area insets Slint reports are zero on a normal phone.

## Colours (`res/values/colors.xml`, `res/values-night/colors.xml`)

| name | day (Android) | night (Android) | day (Slint) | night (Slint) |
|---|---|---|---|---|
| `k_bg` | `#FFF6F6F8` | `#FF121214` | `#F6F6F8` | `#121214` |
| `k_surface` | `#FFFFFFFF` | `#FF1D1D21` | `#FFFFFF` | `#1D1D21` |
| `k_text` | `#FF1B1B1F` | `#FFE6E6EA` | `#1B1B1F` | `#E6E6EA` |
| `k_text2` | `#FF6A6A73` | `#FF9A9AA3` | `#6A6A73` | `#9A9AA3` |
| `k_line` | `#FFE2E2E7` | `#FF2D2D33` | `#E2E2E7` | `#2D2D33` |
| `k_accent` | `#FF3F5F8F` | `#FF8FB0E0` | `#3F5F8F` | `#8FB0E0` |
| `k_on_accent` | `#FFFFFFFF` | `#FF101820` | `#FFFFFF` | `#101820` |
| `k_sel` | `#1F3F5F8F` | `#298FB0E0` | `#3F5F8F1F` | `#8FB0E029` |
| `k_warn_bg` | `#FFF3ECD9` | `#FF3A3426` | `#F3ECD9` | `#3A3426` |
| `k_danger` | `#FFB3261E` | `#FFF2B8B5` | `#B3261E` | `#F2B8B5` |
| `k_chip` | `#FFECEEF3` | `#FF26272D` | `#ECEEF3` | `#26272D` |

`Theme.sel-solid` is k_sel already blended over k_bg (a solid fill for text caps behind a selected
row). Ripple colours (`Ui.pressable`): on a solid button `k_on_accent` at alpha 0x4D; on anything
else `k_text` at 0x26; on list rows `k_text` at 0x22. A disabled control draws its fill and outline
at one third of their alpha.

## Type

| use | size | weight | colour |
|---|---|---|---|
| top bar title | 19 sp | bold | k_text |
| top bar sub line | 12 sp | regular | k_text2 |
| body, empty line, sheet text | 15 sp | regular | k_text2 (sheet), k_text (permission text), k_text2 (empty line) |
| button label | 15 sp | bold | by kind |
| text field | 16 sp | regular | k_text, hint k_text2 |
| list row name | 16 sp | regular | k_text |
| list row date / folder line | 12 sp | regular | k_text2 / accent |
| list row size | 13 sp | regular | k_text2 |
| sheet title | 19 sp | bold | k_text |
| banner | 13 sp | regular | k_text |
| stat tile number / label | 18 sp bold / 12 sp | | |
| bottom-bar tile label | 11 sp | regular | |

## Buttons (`Ui.button`)

15 sp bold, no all-caps, radius 10 dp, at least 44 dp high and 64 dp wide, padding 16 dp
horizontally and 10 dp vertically. While pressed the button squashes to 96 % (90 ms down, 160 ms
back, both scaled by the animation speed) and a ripple grows from the touch point.

| kind | id | fill | outline | label |
|---|---|---|---|---|
| FILLED | 0 | k_accent | none | k_on_accent |
| OUTLINE | 1 | none | k_line, 1 dp | k_accent |
| TEXT | 2 | none | none | k_accent |
| DANGER | 3 | none | k_danger, 1 dp | k_danger |
| TONAL | 4 | k_chip | none | k_accent |

Icon buttons (`Ui.iconButton`): 48 x 48 dp, round ripple (radius 24), 24 dp icon in k_text, same
squash.

## Animation

The animation speed (Settings > Animation speed, preference `anim_level` 0..3, default 2) scales
every duration: Off 0, Fast 0.6, Normal 0.85, Relaxed 1.2 (`Defaults.ANIM_SCALE`). In Slint every
duration is written as `Theme.dur(<base>)`.

| what | base | curve |
|---|---|---|
| press squash down / up | 90 / 160 ms | accelerate-decelerate (`View.animate()` default; Slint `ease-in-out-sine`) |
| ripple grow / fade | 225 / 150 ms | ease-out |
| sheet in (fade + scale 0.92 -> 1) / out | 200 / 140 ms | decelerate / ease-in-out |
| scrim | 0 -> 50 % black, with the sheet | |
| toast visible / fade | 2 s / 150 ms | |
| row slide-in (list) | translate Y 14 % + alpha, 240 / 200 ms, 0.12 stagger | decelerate x1.5 |
| screen open: new screen in | X +12 %, alpha 0 -> 1, 215 / 185 ms | decelerate cubic |
| screen open: old screen out | X -6 %, alpha 1 -> 0.4, 185 ms | accelerate quad |
| screen close: old in / new out | X -6 % -> 0, alpha 0.4 -> 1, 215 / 185 ms; X 0 -> +12 %, alpha 1 -> 0, 185 / 170 ms | |
| fade in / out | 200 / 150 ms | decelerate quad / linear |

## Icons

65 vector drawables in `res/drawable`; the single-path ones (24 x 24 viewport) are
carried over as path data by `tools/gen_icons.py` into `ui/ui/icons.slint`, which `Icon` draws.
`ic_kiln` (the mark) and `ic_launcher_fg` are multi-part and are drawn by `KMark` or kept as the
launcher icon resource. Icon names: `ic_add`, `ic_archive`, `ic_bookmark`, `ic_check`, `ic_close`, `ic_code`, `ic_copy`, `ic_d_android`, `ic_d_bell`, `ic_d_camera`, `ic_d_code`, `ic_d_documents`, `ic_d_download`, `ic_d_heart`, `ic_d_lock`, `ic_d_movies`, `ic_d_music`, `ic_d_phone`, `ic_d_pictures`, `ic_d_sd`, `ic_d_star`, `ic_d_zip`, `ic_delete`, `ic_down`, `ic_edit`, `ic_extract`, `ic_f_apk`, `ic_f_bin`, `ic_f_book`, `ic_f_db`, `ic_f_disk`, `ic_f_doc`, `ic_f_font`, `ic_f_key`, `ic_f_pdf`, `ic_f_sheet`, `ic_f_slides`, `ic_f_term`, `ic_f_text`, `ic_f_web`, `ic_f_zip`, `ic_file`, `ic_folder`, `ic_home`, `ic_image`, `ic_kiln`, `ic_launcher_fg`, `ic_lock`, `ic_more`, `ic_move`, `ic_music`, `ic_new_file`, `ic_new_folder`, `ic_notify`, `ic_open`, `ic_paste`, `ic_refresh`, `ic_search`, `ic_set`, `ic_settings`, `ic_sort`, `ic_up`, `ic_up_dir`, `ic_usb`, `ic_video`.

## Components (`ui/ui/components.slint`)

Each component is a plain Slint element tree with no standard widgets. The line after each name
says which Java it reproduces.

- **Icon** — One of the 63 single-path icons (24x24 viewport). Give it a width and a height.
- **KMark** — The Kiln mark (res/drawable/ic_kiln.xml): arched furnace, glowing mouth, brick base. The drawable is 56x56 and translated by (-26, -24); its colours are fixed.
- **PressArea** — Ui.ripple(): a circle grows from the touch point (225 ms) and fades out (150 ms), clipped to the rounded shape. `pressed` is exposed so the host can squash itself (Ui.pressScale).
- **KEllipsisText** — mode 0: "abc..." (END), mode 1: "ab...yz" (MIDDLE), mode 2: "...xyz" (START). `cap` is the colour behind the text; it hides the half letter next to the dots.
- **KButton** — Ui.button(): kind 0 FILLED, 1 OUTLINE, 2 TEXT, 3 DANGER, 4 TONAL. 15 sp bold, radius 10, at least 44 x 64, padding 16 / 10, squashes to 96 % while pressed.
- **KIconButton** — Ui.iconButton(): 48 x 48, round ripple (radius 24), a 24 dp icon. `mark` shows the untinted Kiln mark (28 dp) instead of an icon.
- **KTextField** — A single-line field: 16 sp, padding 14 / 12, 10 dp corners, 1 dp outline in k_line, k_bg fill.
- **KMeter** — Ui.meter(): an 8 dp bar, 0..1, never shown emptier than 8 / 1000.
- **KTopBar** — Padding 4 / 6, a 48 dp button on each side, the title block in between (pressable, radius 12). Title 19 sp bold (ellipsised in the middle), sub 12 sp (ellipsised at the start).
- **KListRow** — One row: a 40 dp tile (radius 10, k_chip) with a 22 dp icon, the name (16 sp, middle), a date line (12 sp), an optional folder line (12 sp, accent, start-ellipsised) and the size (13 sp). Padding 16 / 9. A selected row is tinted k_sel and shows a check in the tile.
- **KTile** — A bottom-bar tile: an icon over a 11 sp label, padding 10 / 8 / 10 / 6, radius 14.
- **KFab** — The "+  New" button: a FILLED button 48 dp high with a 4 dp elevation, bottom right.
- **KSheetText** — Content of a Sheet: put these (or anything else) inside a KSheet.
- **KSheetOption** — Sheet.option(): padding 8 / 10, radius 12, an optional 38 dp tile with the icon in accent, the label (16 sp), and a ✓ (18 sp bold, accent) when selected.
- **KSheetRadio** — Sheet.radio(): a 22 dp ring (2 dp outline, 5 dp core when selected), the label, a small detail.
- **KSheetDivider** — Sheet.divider(): a 1 dp line with 6 dp above and below.
- **KSheet** — The modal itself. `shown` fades and scales the card in (200 ms, decelerate) and out (140 ms); the scrim goes to 50 % black. The card is at most 460 dp wide, 20 dp from the screen sides, and centred in the room that is left. Buttons: one or two share a row (they stack, main action on top, when the labels would not fit or when there are more than two); `button-clicked` gets the index in the order Sheet.button() would have been called.
- **DetailRow** — Ui.detailCard() row data: a label and a value; `stack` puts the value under its label.
- **KDetailCard** — Ui.detailCard(): k_chip card, radius 14, padding 14 / 4, rows 11 dp apart with 1 px rules.
- **KSwitch** — The stock Material Switch of Theme.DeviceDefault, tinted with colorControlActivated (k_accent): a 36 x 14 dp track and a 20 dp thumb. On: accent thumb, track accent at 30 %. Off: a light grey thumb and a track in the foreground colour at 30 %. The thumb slides in 150 ms.
- **KCheck** — Ui.check(): the stock check box, 15 sp label in k_text, box tinted with k_accent when ticked.
- **KStatTile** — Ui.statTile(): a big number (18 sp bold) over a small label (12 sp), centred, k_chip, radius 14, padding 6 / 12.
- **KToast** — Toast.makeText(..., LENGTH_SHORT): a dark rounded pill near the bottom that fades in and out. The stock toast is drawn by the system, so this is an approximation of its look; the timing is the real one (visible for 2 s, fade 150 ms). Cover the screen with it, set `message` and flip `shown` to true; the host turns `shown` off again after 2 s. It never takes touches.

## Main screen (`MainActivity.buildUi()`), as far as Phase A draws it

A vertical page on `k_bg`:

1. **Top bar** (`KTopBar`): padding 4 dp / 6 dp; a 48 dp button on the left (the Kiln mark at a
   storage root, the "up" arrow elsewhere); the title block (pressable, radius 12, padding 8 / 3):
   title 19 sp bold ellipsised in the middle, a small chevron after it when the bookmarks can open,
   and under it the folder path, 12 sp, ellipsised at the start; then three 48 dp buttons: search
   (a cross while searching), sort, more.
2. *(Search row, hidden until searching: a field "Search" with a magnifier, and a "Subfolders"
   TONAL button. Phase B.)*
3. **Rule**: 1 dp, k_line.
4. *(Banner, hidden: 13 sp on k_warn_bg, padding 16 / 10. Phase B.)*
5. **Body**, a frame with its children stacked, last on top:
   - the list under the pull-down info panel (Phase B);
   - the **empty line**: 15 sp k_text2, centred, 32 dp side padding, at the top of the body
     (56 dp down, or 96 dp when the ".." row is showing). It reads "Starting..." until the engine
     is up and "Kiln couldn't start: <reason>" if it fails;
   - the **"+  New" button**: FILLED, 48 dp high, 16 dp from the right and bottom, 4 dp elevation,
     hidden at first;
   - the *bottom bar* (hidden; Phase B);
   - the **permission view**: opaque k_bg over everything above, 32 dp padding, content centred:
     "Kiln needs access to your files to open, create and unpack archives and to manage files.
     Nothing leaves your phone." (15 sp, k_text, centred), 16 dp gap, then a FILLED button
     "Allow access to files".

### Start-up behaviour (`onCreate`, `startEngine`, `refreshPermission`, `showLastCrash`)

- On start the scratch folders `cache/incoming` and `cache/edit` are deleted; the preview cache
  stays and is trimmed once the engine is up.
- The engine starts on a worker thread. On success the app checks the permission again; on failure
  the empty line shows "Kiln couldn't start: ...".
- Permission is "All files access" (`Environment.isExternalStorageManager()`, always granted before
  Android 11). When missing, the permission view is shown. Its button opens the system page for
  this app (`ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION` with `package:<id>`); the general page
  is the fallback. Returning to the app (`onResume`) checks again, and once access and engine are
  both there the start folder is loaded exactly once.
- If the last run ended in a crash, `files/last-crash.txt` ("Kiln <version>, thread <name>" and the
  trace) is shown once, first 2500 bytes, in a sheet titled "Kiln stopped last time" with one
  FILLED "OK" button (`message(heading, body)`), and the file is deleted.
- Settings are read from `shared_prefs/kiln.xml`, the file the Java app wrote, so they carry over.

### Sheets (`Sheet.java`)

A modal card, centred, at most 460 dp wide and 20 dp from the screen sides, radius 22, k_surface,
1 dp k_line outline, 20 dp padding. Title 19 sp bold (ellipsised in the middle) with 12 dp below;
the body scrolls when it does not fit; 14 dp above the buttons. One or two buttons share a row
(10 dp apart) if both labels fit, otherwise they stack 8 dp apart with the main action on top.
Scrim 50 % black; a tap on the scrim cancels when the sheet is cancelable.

## Main screen, Phase B part 1: the file list (`MainActivity`, `RowView`, `RowAdapter`)

Slint: `ui/ui/browser.slint` (`KFileList`, `RowData`), `ui/ui/main.slint`; Rust: `app/src/browser.rs`
(state and rules), `app/src/browse_ui.rs` (what is shown, what taps do).

- **Row.** Padding 16 / 9, minimum height 58 dp, 12 dp between tile, text and size. Each row is
  followed by a 6 dp gap and is a card 6 dp in from both sides (radius 12; the padding is 6 dp less
  so nothing inside moves). The card dips to 96 % while pressed and once on every tap. The row is
  cached by the renderer (`cache-rendering-hint`), because it does not change while it scrolls.
  Opening another folder fades the rows out (70 ms), swaps them at the top while they are out of
  sight, and fades them in (180 ms) sliding up 10 dp. The tile is
  40 dp, radius 10, k_chip, with a 22 dp icon (9 dp in). Name 16 sp (ellipsised in the middle),
  date line 12 sp k_text2, in a search a third line 12 sp k_accent (ellipsised at the start),
  size 13 sp k_text2 on the right. A selected row is tinted k_sel and shows a check in the tile.
  Every row of a list is as high as the first one (all rows have the same lines), so the list is a
  `ListView` (which builds only the rows in view) of `index * row height` high items.
- **Row text.** Folder: `<date>` and `<n> item(s)`; encrypted set: `Encrypted set · <date>`;
  archive: `Archive · <date>` or `Encrypted archive · <date>` and the size; file: date
  and size. Icon colours: folders and sets k_accent, archives k_text2, files by type (`Icons`).
- **".." row.** Shown when `up_row` is on, the folder is not the storage root, access is granted
  and no search is in progress. Tapping it is Back. The empty-folder line then sits 96 dp down
  instead of 56 dp.
- **Empty line (`empty`).** "This folder is empty.", or with "show all files" off "No Kiln archives
  here.\nOpen the menu to show all files."; "No matches." in a search; "Loading..." only after
  700 ms; "Starting..." until the engine is up; "Kiln couldn't start: <why>".
- **Selection.** Long press selects, taps toggle while anything is selected. The title becomes
  "<n> selected", the left button a cross, sort and search are hidden. The bar (Copy, Move, Delete,
  Pack) is a k_surface card, 1 dp k_line outline, radius 20, 6 dp padding, 12 dp from the sides and
  bottom; it slides up 70 dp while fading in (240 ms) and out (160 ms), both scaled by the
  animation speed. Delete is k_danger, Pack k_accent.
- **"+ New".** 48 dp high, 16 dp from the bottom right, hidden while the list scrolls down by more
  than 6 dp and shown again on the way up; always shown at the top.
- **Search.** The magnifier turns into a cross; a row under the top bar (padding 12 / 0 / 12 / 8)
  holds the field ("Search", focused on opening). The words filter the folder's names (every word
  must occur, case-insensitive). Opening another folder leaves the search.
- **Sort sheet.** "Sort by": Name, Size, Date modified, Type; then Ascending, Descending. Folders
  always come first, ties are by name. The choice is saved to `sort_key` / `sort_asc`.
- **Back (`onBackPressed`).** Leave selection, leave search, go up a folder, then at the storage
  root "Tap Back again to exit" and a second Back within 2 s leaves. Slint's Android backend
  reports Back as a key press (`Key.Back`); the page is wrapped in a `FocusScope` so that a press
  the search field ignores reaches it. Leaving an archive comes with the archive part.
- **No grid view, breadcrumbs or view toggle.** The Phase B brief lists them, but `MainActivity`
  has none (its `grid` is a settings layout), so none is built.
- **No drag-select.** `MainActivity` has none either: a long press on a row toggles that one row
  (`setOnItemLongClickListener`, ignored while a folder is loading) and taps toggle while anything
  is selected. No touch handler selects a range, so none is built.
- **Not in this part** (they come in the parts below): the menus and dialogs (parts 3 to 8),
  archives (parts 9 and 10), opening files and thumbnails (part 11). The service notification is
  not built yet.

## Main screen, Phase B part 2: pull down and bookmarks (`MainActivity`, `PullSwipe`, `SwipeRefreshLayout`)

Slint: `ui/ui/browser.slint` (`KFileList`), `ui/ui/pull.slint` (`KPullPanel`, `KRefreshCircle`),
`ui/ui/marks.slint` (`KMarksPopup`, `MarkRow`), `ui/ui/main.slint`; Rust: `app/src/pull.rs`,
`app/src/marks.rs`, `app/src/prefs.rs` (bookmarks), `app/src/host.rs` (`volumes`, `space`).

- **Info strip (`buildPullPanel`, `fillPull`, `pullMove`, `pullEnd`).** Behind the list, 84 dp
  (`PULL_H`) high, k_surface, padding 16 / 10 / 16 / 0. Two equal columns 12 dp apart of 13 sp
  k_text lines, cut in the middle: left `Type: Storage` or `Type: Folder`, then `Free: <free> / <total>`
  (empty when the phone cannot say); right `Storage: <name>` or `Folder: <name>`, then `Size: ...`,
  which climbs as the folder is counted (every 120 ms, `Size: <n>...`) and ends as `Size: <n>`.
  Under them, 10 dp down, a 6 dp bar: k_line track, k_accent fill, `used / total` and never below
  8 / 1000 once the space is known. The count walks the folder on a worker thread (at most 40 levels
  deep), stops when the strip is gone (500 ms after the finger lets go) or a new pull begins.
- **Pulling.** Only at the top of the list, with nothing selected and access granted, and only
  when the list was already resting there (for 180 ms) before the finger pulled: a fast scroll that
  runs into the top is stopped there and neither opens the strip nor refreshes. The list
  (and the empty line) move down by 0.6 of the finger's travel, at most 84 dp; the strip is as tall
  as that and fades in over the first 42 dp. On release they slide back in 220 ms (scaled by the
  animation speed), after a 250 ms wait when the strip was at least 90 % out; the easing is
  ease-in-out (the ViewPropertyAnimator default).
- **Refresh (`onPull`, `SwipeRefreshLayout`).** Works with a selection too. The circle is 40 dp,
  k_surface, 4 dp elevation, a 7.5 dp ring 2.5 dp thick in k_accent. The library's drag rate is 0.5
  and its trigger distance 64 dp, so the pull must be longer than 128 dp. While the finger moves
  the circle follows the library's curve (`moveSpinner`: top = -40 + 64 * drag% + the slingshot
  term, plus the circle's `offset`, which keeps it below the usage bar of the strip), the ring grows to 0.8 of a turn with an arrow at its end, drawn at 30 % until the pull is
  long enough and then at full strength (300 ms). On release the circle either rises out of view
  (200 ms) or settles 24 dp (plus the offset) from the top and turns (1332 ms a cycle) until the folder has been read
  again (`applyScan`: `setRefreshing(false)`). The folder is read with the selection kept. The
  values are those of androidx `SwipeRefreshLayout` 1.1 (not in this repository).
- **Bookmarks (`showBookmarks`, `markRow`, `actionRow`, `goTo`).** Tapping the title block (when
  access is granted and nothing is selected) opens a card under it; tapping it again, anywhere
  else, or Back closes it. The frame is `min(screen - 8 dp, 360 dp)` wide, starts 4 dp left of the
  screen edge with 8 dp padding at the sides, 2 dp above and 14 dp below the card. The card:
  k_surface, 1 dp k_line outline, radius 18, 6 dp padding, 8 dp elevation, at most 60 % of the
  screen high (it scrolls). It fades and grows from the top (90 % to 100 %) in 170 ms
  (decelerate 1.6, scaled by the animation speed) and closes at once.
  Lines: Internal storage (home icon), every mounted USB drive or SD card (usb icon; name from the
  phone), then the saved folders (in the order they were added; the folder's own icon, its name or,
  for a root, its path). A line has padding 8 / 7 / 4 / 7, a 36 dp tile (radius 10, k_chip, a
  22 dp accent icon) and 12 dp, the name (15 sp, cut at the end) over the path (11 sp k_text2,
  cut at the start), and on the right 12 sp k_text2 `<free> free` / `of <total>` for a storage.
  A bookmark ends in a 40 dp cross; a storage in 8 dp. The folder on screen has a bold accent
  name. Below the lines, unless at a storage root, a 1 dp rule with 4 dp above and below and one
  line: `Bookmark this folder` (add icon) or `Remove this bookmark` (cross icon), which toast
  `Bookmarked <name>` or `Bookmark removed`. Tapping a line goes there (`That folder isn't there any
  more` if it is gone; nothing if it is already open); the cross removes the bookmark
  (`Bookmark removed`). Tapping a bookmark's tile, or long-pressing it, opens the icon picker (see part 5 below).
- **Storages.** A USB drive or SD card is a storage root like the phone's own storage: the title
  is its name, the left button is the Kiln mark, the ".." row is not shown there and Back at its top
  asks to exit. Root (`/`) belongs with Shizuku and root (Phase C).
- **Settings file.** Bookmarks are the JSON array `bookmarks` in `kiln.xml`, as `Defaults.java`
  writes it (slashes escaped); picked folder icons are read from the JSON object `folder_icons`.

### Differences from the Java in this part

- **Pulling is done by the list.** A Slint `Flickable` cannot be dragged past its top, so the list
  keeps 140 dp (84 dp / 0.6) of empty content above its first row, rests scrolled by that much, and
  dragging down uses it up; the list is drawn 0.6 of that far down. The finger being down is read
  from a `TouchArea` around the list; if the pointer never reports it, the pull lets go 150 ms after
  the last movement (and in any case after 2 s).
- **The circle** shows the ring's growth, the arrow and the turn as the library does, but the turning
  ring is an approximation of `CircularProgressDrawable` (its sweep goes from 0.1 to 0.8 of a turn
  once per cycle), and when the folder has been read it rises out of view (200 ms) instead of
  shrinking (150 ms).
- **Rows do not slide in yet** (`Ui.rowAnim`, on a new folder and on a refresh); that comes with the menus.

## Main screen, Phase B part 3: the dialog sheet and the "more" menu (`Sheet`, `MainActivity.showMenu`, `menuSheet`, `onMenu`)

Slint: `KSheet` and `KSheetOption` (`ui/ui/components.slint`), the dialog block of `ui/ui/main.slint`
(`DlgOption`); Rust: `app/src/dialog.rs` (`Spec`, `Btn`, `Opt`), `app/src/browse_ui.rs` (`show_menu`),
`app/src/browser.rs` (`select_all`, `only_selected`).

- **Long press has no menu.** `setOnItemLongClickListener` only toggles the row's selection. The
  menu is the "more" button's.
- **Dialog sheet (`Sheet`).** Content goes in this order: body text, option rows, an optional field,
  then the buttons. A title that is empty shows no title line. One sheet is up at a time; showing
  another replaces it.
- **Option row (`Sheet.option`).** Tapping it closes the sheet and runs its action. Here the action
  runs first and the sheet closes afterwards unless the action showed another dialog, which looks
  the same. Choosing a row is not a back-out, so the "go back" action does not run.
- **Back-out.** Back, a tap outside the card, or a button with no action (Cancel, OK) closes the
  sheet and runs the sheet's "go back" action (`Sheet.nextBack`; `Spec.back` in Rust). A menu sets
  it before opening a sheet from one of its rows, so that backing out of that sheet shows the menu
  again. Menu rows that open dialogs will set `back` to `show_menu` when those dialogs exist.
- **Enter in a field.** Nothing. `Ui.field` is a single-line `EditText` and `Sheet` adds no
  editor-action listener, so the key only does what the keyboard's own action does (hides it).
  The Slint field's `accepted` is left unconnected on purpose.
- **Menu rows** (icon in a 38 dp accent tile, label 16 sp). Nothing selected: Select all (check),
  Show archives only / Show all files (file), Hide hidden files / Show hidden files (file),
  Refresh (refresh), Recycle bin (delete), Settings (settings), About (archive). Selecting: Select
  all, Properties (file), Rename (edit; one selected), Icon and color... (folder; one folder),
  Open options... (open; one plain file), Extract... (extract; one archive or set), Browse as plain
  folder (folder; one set), Clear selection (close). The menus of an open archive come with the
  archive part.
- **Built.** Select all adds every row shown. Clear selection. The two Show toggles flip
  `show_all` / `show_hidden`, save them to `kiln.xml` and read the folder again (selection cleared).
  Refresh reads the folder again and keeps the selection. Browse as plain folder opens the set as an
  ordinary folder (with the banner). Icon and color... opens the folder icon picker (see part 5 below). Properties opens the properties sheet (see part 6 below). Open options... (one plain
  file selected) opens the file sheet (see part 11 below), and Extract... (one archive selected)
  opens the archive and then its Extract form (see parts 9 and 10 below). Settings is the one row
  that still toasts "Not in this build yet" (Phase C).
- **Rename** is also hidden in a protected (read-only) folder in the Java; that rule comes with
  Root and Shizuku (Phase C).

### Differences from the Java in this part

- **Keyboard.** The Java window resizes with the keyboard, keeps the card centred above it and
  glides the picture there in 260 ms (decelerate 1.8, half of the layout shift). The Slint card
  only follows `safe-area-insets`; there is no separate glide.
- **Field.** It exists only while the sheet is shown, so it vanishes when the close fade starts and
  the card shrinks during the 140 ms fade-out. The Java keeps the view until the fade ends.
- **Refresh** from the menu does not slide the rows in again (`forceAnim`); that comes with the
  row animations.

## Main screen, Phase B part 4: the selection bar and Delete (`MainActivity.deleteSelected`, `deleteForGood`)

- **Bar.** While rows are selected: Copy, Move, Delete (danger tint) and Pack tiles. All four are
  built (Pack opens the Pack form, see part 7 below). The Java shows a
  single Copy tile in a protected (read-only) folder; that rule comes with Root and Shizuku
  (Phase C). The archive's bar is described in part 9 below.
- **Delete.** A dialog "Delete <what>?" (what is the quoted name for one item, "N items" for more),
  the text "This can't be undone.", Cancel (text) and Delete (danger). Delete runs `fs_delete` on a
  worker thread. A cancelled run toasts "Cancelled"; a failed one opens the message sheet "Couldn't
  delete everything" with the engine's message. Either way the folder is read again (selection
  cleared).

### Differences from the Java in this part

- **Progress sheet** (`runOp`, `ProgressUi`; `run_op` in dialog.rs, `prog-*` in main.slint).
  Copying, Moving, Deleting, Moving to recycle bin, Emptying recycle bin and Deleting one item
  from the bin run behind a modal sheet that opens only when the work lasts 250 ms. It has the
  heading as its title ("Copying...", "Moving...", "Deleting...", "Moving to recycle bin...",
  "Emptying recycle bin..."), a line for the current file (14 sp, muted) with the percent (16 sp
  bold) to its right, a bar (`KProgressBar`: 4 dp, accent on the line colour, a sliding segment
  while the progress is unknown) and an outline Cancel. Back and a tap outside do nothing.
  Cancel stops the job at its next check, turns the line into "Cancelling...", greys Cancel out
  and keeps the sheet up until the work returns; the usual result follows ("Cancelled", or
  "Cancelled after N items" when some items had gone to the bin). Cancel on the one "Delete
  permanently" of the bin does nothing, as in the Java. A second operation started while one
  runs is ignored. Differences: no "Keep the screen from turning off" box (the platform crate
  has no call for it yet); the bar jumps to each report where the Java eases it and creeps
  between reports; a long file name is cut at the end, not the middle; no notification, no
  foreground service after 1.5 s and no notification permission request (B18); the bin jobs send
  no progress, so their bar slides; the 4 dp bar in a 16 dp box is the Material default, not
  measured in the Java.
- **Recycle bin.** Delete follows `delete_mode` like `deleteSelected` (0 ask, 1 recycle bin, 2
  permanent). Settings, where it is changed, is Phase C, so it stays at the default, ask: "Delete
  <what>?" with the rows "Move to recycle bin" and "Delete permanently". Only files on Internal
  storage or a mounted drive can go to a bin; anything else is deleted for good after the confirm
  "These can't go to the recycle bin here, so this can't be undone." Moving to the bin shows the
  "Moving to recycle bin..." progress sheet (see **Progress sheet** above).
- **Recycle bin sheet** (`binSheet`, `showBin`, `binItem`). Menu > Recycle bin lists the bins of
  Internal storage and every mounted drive, newest first: "N items \u00b7 size", then a row per item
  (its name; "size \u00b7 date" or "Folder \u00b7 size \u00b7 date"; the folder it came from) and Close and
  Empty. With nothing in it: "The recycle bin is empty." and Close. A row opens the item: Restore
  (toast "Restored to <folder>", under "name (restored).ext" when the name is taken) and Delete
  permanently ("Delete "name" for good?"). Back and Cancel return to the list. Empty asks "Empty
  the recycle bin?". `KSheetOption` gained a `sub` line (12 sp, muted) for these rows. Differences:
  the whole sheet body scrolls (the Java has a fixed 330 dp list under fixed text); a long name is
  cut at the end, not the middle. Emptying shows the progress sheet "Emptying recycle bin..." and
  deleting one item "Deleting..." (see **Progress sheet** above).
- **Copy and Move** (`setClip`, `paste`, `runTransfer`, the paste bar of `buildUi`). Copy or Move
  holds the selected items (the selection is cleared) and toasts `Open a folder, then tap "Paste
  here"` (`"Move here"` for Move). While something is held and nothing is selected the bar is the
  paste bar: "Copying N items" or "Moving N items" (15 sp bold), "Into <folder, or the storage's
  name>" (12 sp), then New folder, Cancel and Paste here / Move here; the "+ New" button is
  hidden. Paste here asks the engine for a plan (`fs_plan`); if it fails: "Can't copy" or "Can't
  move" with the reason. If nothing is in the way it runs `fs_transfer` on a worker, toasts the
  engine's message ("Cancelled", or a "Not everything was copied/moved" sheet on failure), lets go
  of the items and reads the folder again. When names already exist in the folder (`paste`), a
  sheet titled "1 item already exists" or "N items already exist" lists the first five as
  "\u2022 name" lines and "and N more", with the rows Replace, Skip existing and Keep both (no
  icon tile; each runs `fs_transfer` with policy replace, skip or keep) and Cancel (text; the
  items stay held). Differences: the rows without an icon have not been compared pixel by pixel
  with the Java; the read-only rule ("Can't paste here") comes with Phase C. The transfer shows
  the "Copying..." or "Moving..." progress sheet (see **Progress sheet** above).
- **Field errors.** The Java shows `EditText.setError` (red icon and a popup). The Slint dialog shows
  the text as a red line under the field (`dlg-error`).

## Main screen, Phase B part 5: the folder icon picker (`MainActivity.iconPicker`, `refreshDirIcons`, `dirIcon`)

- **Where it opens.** "Icon and color..." in the selection menu, for one selected folder: the
  selection is cleared, then the sheet opens with the colour swatches. Also a bookmark's icon
  tile, or a long press on a bookmark line in the drop-down: the drop-down closes, the sheet has
  no swatches, and the drop-down opens again after an icon or "Use default" was chosen.
- **Sheet.** Title "Icon and color for <name>" (with swatches) or "Icon for <name>". Buttons
  "Use default" (outline; clears the icon, and the colour when the swatches are shown) and
  "Cancel" (text). Cancel, Back and a tap outside the card close it and change nothing.
- **Swatches** (`KSwatch`). 11 cells, 6 to a row, the last row padded: "default" first (drawn in
  the accent, saves 0), then 0xFFE5574B, 0xFFE8833A, 0xFFE0A030, 0xFF5DB75D, 0xFF3FA6A0,
  0xFF4A8FE7, 0xFF5B6EEF, 0xFF8E7CC3, 0xFFC45BA3, 0xFF8D99AE. A cell is 42 dp tall, a 34 dp circle
  centred; the current one has a 3 dp outline in k_text and a white tick (16 sp bold), the others
  1 dp in k_line. A tap saves the colour, moves the tick, re-tints the icons below and updates the
  list behind; the sheet stays open. A saved colour that is not in the list ticks nothing.
- **Icons** (`KPickCell`). 4 to a row, 18 in `Icons.FOLDER_KEYS` order (Folder, Download, Camera,
  Pictures, Movies, Music, Documents, Android, Sounds, Archives, Favourite, Heart, Private, Code,
  Home, Phone storage, SD card, USB drive), the last row padded. A cell has 2 dp around it, radius
  12, padding 2 / 8 / 2 / 6, a 40 dp icon in the picked colour (else the accent) over the label
  (11 sp, one line, cut at the end). The current icon has a k_chip fill, an accent outline and a
  bold label in k_text; the others use k_text2. A tap saves the key and closes the sheet.
- **Rows.** A folder row on screen (not an encrypted set) shows the picked icon and colour; the
  bookmark lines show the picked icon. The list is drawn again after every change.
- **Storage.** `folder_icons` (path to key) and `folder_colors` (path to ARGB as a signed 32-bit
  number, like a Java `int`; 0 removes the entry), both JSON text with "\/" for "/". Renaming or
  moving a folder moves both for it and its subfolders.

### Differences from the Java in this part

- The outline of the current icon cell is 1 dp (`Theme.hair`) and there is no gap between the
  icon and its label; neither was read from the Java.
- The swatch and icon cells use the standard `PressArea`; the exact ripple of `Ui.pressable` on
  them was not compared. The swatch press shape is a 21 dp circle.
- The icon grid always has 5 rows, which is what 18 icons need at 4 to a row.
- The picker's Slint (nested `for` loops, a `\u{2713}` tick) was written without a build; check it
  in CI before anything else in this part.

## Main screen, Phase B part 6: the Properties sheet (`Props`, `MainActivity.propertiesSelected`)

- **Where it opens.** "Properties" in the selection menu, for any selection of files and folders.
  Back closes the sheet.
- **Buttons.** "Copy" and "OK". Copy puts the rows on the clipboard as text (the host's
  `set_clipboard`, `ClipboardManager.setPrimaryClip` on Android) and toasts "Copied".
- **One item.** A name line, then the heading "General information" with these rows: Type, File
  count (folders), Size, Last modification, Permissions (when the file system has them), FS path and
  FS type (when the mount is known) and MD5 (files). A folder shows "Calculating..." for File count
  and Size until a background walk has counted it; a file shows "Calculating..." for MD5 until the
  digest is done ("unavailable" if the file cannot be read). An archive or a set adds the heading
  "Archive properties" with Type, Encrypted, File count, Content size and Compression ratio, filled
  in by a background probe. A folder also shows the `.nomedia` check box.
- **Background work.** The walk, the MD5 and the archive probe run on worker threads that post
  their answers to the UI thread. Closing the sheet, or opening it again, sets a stop flag, and
  answers that arrive after that are dropped.

### Differences from the Java in this part

- Values are plain text and cannot be selected. Copy is the way to take them.
- There is no 300 ms wait before late answers change the sheet.
- "Last modification" is empty for a time of 0, where the Java prints 1970.
- The MIME table is partial compared with Android's `MimeTypeMap`.
- The properties box inside an archive (`Props.showInArchive`) is not built. It waits for the
  archive part, because there is no open-archive state yet.
- In the Java a set is a folder, so its "Archive properties" block never runs. It is ported as is.
- The sheet and `props_ui.rs` were written without a full build of the app crate; check them in CI
  before anything else in this part.

## Main screen, Phase B part 7: the Pack sheets (`MainActivity.packDialog`, `runPack`, `packResult`)

- **Where it opens.** The Pack tile of the selection bar, for any selection. The sheet is titled
  `Pack "name"` for one item and `Pack N items` for more. Back closes the form, and the result sheet too.
- **Fields.** Name (the first item's name without its extension plus ".kiln"; for several items
  "archive" in the storage root, otherwise the name of the current folder), Save in (the current
  folder) with a Browse button, then the check boxes. "Encrypt with a password" is on; only while it
  is on, Password (8+ characters), Repeat password, "Show passwords" and "Don't compress (faster for
  photos, music, video)" show under it. Then "Separate archive for each file" and "Delete original
  files after packing" (it starts from the saved default, off when none is saved). Buttons: Cancel
  (text) and Create (filled).
- **Name and note.** Ticking "Encrypt" or "Separate archive" runs the Java's `sync`. With Separate on,
  the ending ".kiln" is cut off; with it off, the ending is put back (unless the name is empty). A
  note under the boxes says what Separate does: "Creates a folder. File names are replaced by random
  names; open the folder in Kiln to see the real ones." with encryption, "Creates a folder with one
  archive per file." without.
- **Create.** The fields are checked in this order, and the first failure keeps the sheet open with
  a red line under its field: a name that is empty or has a slash ("Enter a name (no slashes)"), a
  folder that is not there ("That folder doesn't exist"), and, with Encrypt on, a password shorter
  than 8 ("At least 8 characters") or a repeat that differs ("Passwords don't match"). If the target
  already exists, a dialog `Replace "name"?` comes first, with "It already exists.", Cancel (text)
  and Replace (danger).
- **Run.** The progress sheet "Packing..." (cancellable) runs the engine's `pack` on a worker thread
  with the items, the target, the options, the password and the thread count 0. When it ends:
  ok clears the selection, reads the folder again and opens the result sheet; a cancelled run toasts
  "Cancelled"; anything else opens the message sheet "Couldn't create the archive" with the engine's
  message.
- **Result sheet.** A head with an accent-tinted icon (set for separate archives, lock when
  encrypted, archive otherwise), the heading "Archive created" or "Archives created" and the name of
  the target. When the engine's closing line can be read, three tiles follow (the count, with
  "archive(s)" or "file(s) packed"; the stored size; the time in seconds) and a Compression card:
  "Saved N%" or "No saving", a meter and "<original> became <stored>". When it cannot be read, a
  "Result" row shows the line ("Done" if it is empty). The detail rows are Location, Layout, Encryption,
  Parallel (the first bracket of the line that mentions cores or threads), Compression ("Stored
  only", when set) and Originals ("N files deleted", plus ", M couldn't be deleted"; only when
  "Delete original files" was ticked). OK closes the sheet.

### Differences from the Java in this part

- Browse (`pickFolder`) opens the folder chooser (see part 10 below).
- There is no `Priv.involves` check ("Can't pack here yet"); it comes with Root and Shizuku (Phase C).
- The thread count is always Automatic (jobs = 0); Settings is Phase C.
- No autofocus on the Name field. A long name in the result sheet is cut at the end, not in the middle.
- Each form field has its own red error line (the Java uses `EditText.setError`). The tint of the
  result sheet's icon tile is 0.15 (the Java's 0x26).
- Ticking "Separate archive" turns "x.kiln" into "x." (the Java cuts four characters). It is ported as is.
- The password length counts characters, not UTF-16 units.
- The engine's closing line is read from its start; the Java's regex finds it anywhere in the message.
- The form, the result sheet and `pack_ui.rs` were written without being seen on screen; check them
  on a device. The line reader has unit tests.

## Main screen, Phase B part 8: the About sheet (`MainActivity.aboutDialog`, `showAbout`)

- **Where it opens.** "About" in the menu with nothing selected.
- **Head.** The Kiln mark in a 72 dp k_chip tile (radius 20, 14 dp padding), "Kiln" (24 sp bold), a
  chip "Version x" (13 sp bold, accent, k_chip, radius 12, 12 / 4 dp padding) and the line "Open,
  create and unpack Kiln archives, and manage ordinary files." (14 sp, centred).
- **Details.** A detail card: Version ("x", plus " (build N)" when N is above 0), Engine ("kiln x")
  and Fast mode ("On", or "Off – <reason>"), both only when the engine answered, and Encryption
  (AES-256-GCM). The engine's `version` is asked on a worker thread; the sheet opens when it answers.
- **Button.** OK closes the sheet. A tap outside the card does too.

### Differences from the Java in this part

- The tagline has no extra line spacing (the Java sets 1.15).
- The build number comes from `Host::version_code` (`PackageInfo.getLongVersionCode()`).
- The sheet was written without being seen on screen; check it on a device.

## Archive browsing, Phase B part 9: an open archive (`openItem`, `doOpen`, `onOpened`, `guardLeave`, `saveArchive`, `discardArchive`)

Rust: `app/src/arc_ui.rs` (what is shown, what taps do) and `app/src/arc.rs` (the archive's state and
the readers of the engine's lines); the bar and the sheets are drawn by `ui/ui/main.slint`.

- **Opening.** Tapping an archive row, or Extract... for a selected archive (part 3), reads its
  contents behind a progress sheet headed "Opening..." ("Unlocking..." once a password was given).
  A set (an encrypted folder) and an archive with a lock icon ask for the password first: the sheet
  "Unlock <name>" has a Password field, "Show password", Cancel and Unlock; Unlock with nothing typed
  says "Enter the password". A wrong password toasts "Incorrect password" and asks again. Any other
  failure is the sheet "Couldn't open <name>" with the engine's message.
- **Inside.** The rows are the folders and files directly in the folder shown; a folder adds up what
  is below it. Tapping a folder enters it, tapping a file opens its sheet (part 11). Back goes up one
  folder and lands where that folder was left scrolled; Back at the top leaves the archive for the
  folder it was opened from. Searching inside an archive matches the words against file names, at
  most 1000 hits, each shown with the folder it lives in.
- **Changes.** The first change (Rename, for one) starts an editing session behind "Preparing...";
  what is changed stays in it until it is saved.
- **Unsaved changes.** An archive with changes shows an "Unsaved changes" row above the bar, and
  Save changes and Discard changes at the top of its menu. Leaving it (Back at the top, a file
  opened from another app, Browse as plain folder) first asks "Unsaved changes": "This archive has
  changes that haven't been saved." with Keep editing, Discard and Save. Save runs behind
  "Saving..." and toasts "Saved" (a failure is "Couldn't save"). Discard asks "Discard changes?"
  ("Everything you changed since the last save is thrown away.") and then runs behind
  "Discarding..." (a failure is "Couldn't discard").
- **Menu.** Nothing selected: Save changes and Discard changes (only with changes), Extract all...,
  Check integrity, Select all, Archive info and, for a set, Browse as plain folder and Decrypt folder
  in place.... Rows selected: Rename (one row), Properties, Select all, Clear selection.
- **Bar.** Four tiles. Nothing selected: Extract all, Add, Check, Info. Rows selected: Extract, Copy,
  Move, Delete (danger tint). Extract all, Extract, Check and Info are built (part 10); Add, Copy,
  Move and Delete toast "Not in this build yet".

### Differences from the Java in this part

- `Priv` (Root and Shizuku) is Phase C: no archive is a locked copy, so Rename is always offered,
  and saving never goes through `Priv`.
- The Add, Copy, Move and Delete tiles of the bar are not built yet.
- Written from the Java source; not compared with it on a device.

## Archive tools, Phase B part 10: Extract, Check, Info and the folder chooser (`extractDialog`, `runExtract`, `runTest`, `infoDialog`, `decryptInPlace`, `pickFolder`)

Rust: `app/src/extract_ui.rs` and `app/src/folder_ui.rs`; the sheets are in `ui/ui/main.slint`.

- **Extract form.** Opened from the archive menu (Extract all...), the bar (Extract all, or Extract
  for the selected rows), a file's sheet (Extract...) or the folder list's selection menu (which
  opens the archive first). The text says "Extract all N files to:" or "Extract N file(s) to:", and
  under it are the folder field, Browse and the box "Replace files that already exist". The folder
  starts beside the archive, named like the archive without its extension ("<name>_files" for a
  set); for a file opened from another app it starts in the folder that file came from, or in the
  storage root when it was shared. Browse opens the folder chooser at the folder in the field and
  writes the pick back. Buttons: Cancel and Extract; an empty folder says "Choose a folder". With
  unsaved changes the form is replaced by the sheet "Unsaved changes" ("Save or discard your
  changes first, then extract.").
- **Running it.** The engine's `extract` runs behind the progress sheet "Extracting..." (it can be
  cancelled: toast "Cancelled"). When files already exist and the box was not ticked, the sheet
  "Replace existing files?" has Cancel and Replace; Replace runs it again, overwriting. Any other
  failure is the sheet "Couldn't extract" with the engine's message.
- **Result sheet.** Tiles for the number of files ("file" or "files") and the time ("N s"), then
  detail rows: Result, Saved to, From, Verification ("Every checksum matches", when the engine says
  all checksums are OK) and Existing files ("Replaced", when overwriting). OK closes it; Open folder
  leaves the archive and shows the folder the files went to.
- **Check.** Check integrity (menu) or the Check tile runs the engine's `test` behind "Checking..."
  (it can be cancelled). With unsaved changes it shows the sheet "Unsaved changes" ("Save your
  changes first; Check works on the saved archive."). The result sheet has a verdict, tiles (files
  checked, time) and detail rows: Result ("Every checksum matches", the engine's text, or "No
  problems found"), Encryption (AES-256-GCM or None) and Files in archive. A failed check shows
  "What went wrong" and "Details" instead, and the warning "The archive may be damaged. Keep your
  original files until you've restored or re-created it."
- **Info.** Archive info (menu) or the Info tile: a tile with the file count, what was saved
  ("Saved <size> (<n>%)", or "No saving") with the caption "Stored size is <n>% of the original",
  a line that says "Encrypted" and "AES-256-GCM" joined by a middle dot, or "Not encrypted", and
  detail rows: Type ("Archive" or "Encrypted set (one archive per file)"), Folders, Modified, Size
  on disk and Location.
- **Decrypt folder in place...** (sets only). The sheet "Decrypt in place?" says the folder becomes
  an ordinary folder and that the encryption and the random file names are removed; Cancel and
  Decrypt. It runs behind "Decrypting...", toasts "Decrypted" and closes the archive; a failure is
  the sheet "Couldn't decrypt".
- **Folder chooser (`pickFolder`).** A sheet with the path, the folders of one directory, "New
  folder..." and the other storages. "Select this folder" gives the folder back to whoever asked
  (Extract, and Pack's Browse).

### Differences from the Java in this part

- `Priv` is Phase C: no destination counts as protected, and no folder is read through it.
- Written from the Java source; not compared with it on a device.

## Opening files, Phase B part 11: Open, edit, files from other apps and thumbnails (`openFile`, `fileSheet`, `appChooser`, `openIncoming`, `Thumbs`)

Rust: `app/src/open_ui.rs`, `app/src/host.rs` (what the phone does, the `Host` trait),
`platform/src/android.rs` (the JNI calls, the only code that can run only on a phone),
`app/src/thumbs.rs` and `app/src/thumbs_ui.rs`.

- **Tapping a plain file (`openFile`).** The app saved for its extension (`def_<ext>` in `kiln.xml`,
  stored as "package|label") opens it at once. With none saved, or with a value for Kiln's own
  viewer or editor (Phase C), the file sheet is shown. A saved app that is gone is forgotten:
  toast "That app isn't available any more", then the file sheet. The type sent to the other app is
  the one Android knows for the extension, else what the content looks like.
- **File sheet (`fileSheet`).** Titled with the file's name; the text is the size and the date
  joined by a middle dot. Rows: "Open with..." and "Edit with...", then Cancel. The same sheet opens
  from "Open options..." in the selection menu (one plain file selected); the selection is cleared
  first. The "in Kiln" rows wait for Phase C.
- **App chooser (`appChooser`).** Titled "Open with" or "Edit with", it lists the apps that can open
  (or edit) the file, then "More options..." for the system's own list. For Open with a file that
  has an extension, the first row is "Always open .<ext> files with this app"; tapping it flips its
  tick and shows the sheet again, and an app chosen while it is ticked is saved for that
  extension. Failures toast "That app couldn't open the file" or "No app can open this file".
- **Handing the file over.** The other app gets a `content://` URI from `MediaStore.Files` (found
  by the file's path). When the media scanner has no row for it, a `file://` URI is used instead
  (some receiving apps cannot read it).
- **A file inside an archive.** Its sheet is titled with the file's name; the text is the size and,
  when known, the date. Rows: "Open with..." (the file is extracted to the cache behind "Opening...",
  which can be cancelled, then the chooser opens), "Edit with..." (an editing session, then the
  file is copied out behind "Preparing..." and the chooser opens for editing) and, when nothing is
  waiting to be saved, "Extract...". When the app comes back to the front and the copy that went
  to an editor changed, the sheet "Update the archive?" asks whether to put the new version in
  ("<name>" was changed. Put the new version in the archive? (You still need to Save.)); Update runs
  behind "Updating..." (a failure is "Couldn't update").
- **Files from other apps (`openIncoming`).** When another app opens an archive with Kiln, it is
  opened once storage access and the engine are ready, after the question about unsaved changes if
  an archive with changes is open. A `file://` file is opened where it is, and Extract starts
  beside it. A `content://` file is copied to the cache (behind "Opening...") and then opened;
  Extract starts in the storage root. Older copies are removed, and leaving the archive removes
  its copy. A failure is the sheet "Couldn't open the file".
- **Thumbnails (`Thumbs`).** A row gets a preview in place of its icon (the icon stays while the
  row is selected). Pictures (png, jpg, jpeg, gif, webp, bmp, heic, heif) up to 15 MB, the cover art
  inside audio files (mp3, m4a, flac, ogg, oga, opus, wav, aac, wma, mka, amr) and a frame one
  second into videos (mp4, mkv, webm, 3gp, mov, avi, m4v, ts). Only the rows in view are decoded,
  by two worker threads, at 96 dp and never above one million pixels; results are kept in a cache
  of at most 32 MB and 2048 entries. A file that cannot be decoded keeps its icon. There are no
  previews inside archives or when the setting `previews` (on by default) is off.

### Differences from the Java in this part

- Sharing (an `ACTION_SEND` row) is not built: `Host` can open a file but not send one.
- "View in Kiln" and "Edit in Kiln" are left out until Phase C.
- The foreground service that keeps an operation alive in the background (`OpService`) is not built.
- Written from the Java source; not compared with it on a device.

## Known differences from the Java

- **Toast.** Android draws its own toast; `KToast` approximates its look and uses its timing.
- **Switch and check box.** The Java uses the stock Material controls of `Theme.DeviceDefault`;
  `KSwitch` and `KCheck` reproduce their size, colours and motion.
- **Screen transitions.** The Java opens screens as activities with the slide above. In one window
  the same motion is done by the UI (Phase B); the table lists the values.
- **dp rounding** and **fonts on phones without Roboto**, as noted above.
