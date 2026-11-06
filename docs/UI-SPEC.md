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

- **Row.** Padding 16 / 9, minimum height 58 dp, 12 dp between tile, text and size. The tile is
  40 dp, radius 10, k_chip, with a 22 dp icon (9 dp in). Name 16 sp (ellipsised in the middle),
  date line 12 sp k_text2, in a search a third line 12 sp k_accent (ellipsised at the start),
  size 13 sp k_text2 on the right. A selected row is tinted k_sel and shows a check in the tile.
  Every row of a list is as high as the first one (all rows have the same lines), so the list is a
  `Flickable` that builds only the rows in view (plus 3) at `index * row height`.
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
- **Not yet in this part** (the Java has them; see the Phase B list): the menus and every dialog, file operations, thumbnails, opening files, archives, the
  service notification. Tapping a file says "Not in this build yet".

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
- **Pulling.** Only at the top of the list, with nothing selected and access granted. The list
  (and the empty line) move down by 0.6 of the finger's travel, at most 84 dp; the strip is as tall
  as that and fades in over the first 42 dp. On release they slide back in 220 ms (scaled by the
  animation speed), after a 250 ms wait when the strip was at least 90 % out; the easing is
  ease-in-out (the ViewPropertyAnimator default).
- **Refresh (`onPull`, `SwipeRefreshLayout`).** Works with a selection too. The circle is 40 dp,
  k_surface, 4 dp elevation, a 7.5 dp ring 2.5 dp thick in k_accent. The library's drag rate is 0.5
  and its trigger distance 64 dp, so the pull must be longer than 128 dp. While the finger moves
  the circle follows the library's curve (`moveSpinner`: top = -40 + 64 * drag% + the slingshot
  term), the ring grows to 0.8 of a turn with an arrow at its end, drawn at 30 % until the pull is
  long enough and then at full strength (300 ms). On release the circle either rises out of view
  (200 ms) or settles 24 dp from the top and turns (1332 ms a cycle) until the folder has been read
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
  (`Bookmark removed`). Tapping a bookmark's tile, or long-pressing it, opens the icon picker; that
  dialog comes with the dialogs, so for now it says `Not in this build yet`.
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
  ordinary folder (with the banner). Every other row toasts "Not in this build yet" until its
  dialog is built.
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

- **Bar.** While rows are selected: Copy, Move, Delete (danger tint) and Pack tiles. Only Delete is
  built; Copy and Move toast "Not in this build yet" until B6, Pack until B21. The Java shows a
  single Copy tile in a protected (read-only) folder; that rule comes with Root and Shizuku
  (Phase C), and the archive bars come with the archive part.
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

## Known differences from the Java

- **Toast.** Android draws its own toast; `KToast` approximates its look and uses its timing.
- **Switch and check box.** The Java uses the stock Material controls of `Theme.DeviceDefault`;
  `KSwitch` and `KCheck` reproduce their size, colours and motion.
- **Screen transitions.** The Java opens screens as activities with the slide above. In one window
  the same motion is done by the UI (Phase B); the table lists the values.
- **dp rounding** and **fonts on phones without Roboto**, as noted above.
