//! The Files tab and the folder screen of the new UI: what they show, and what a tap does.
//!
//! A folder is a [`kiln_state::Listing`] behind a Slint model ([`ListingModel`]) that builds a
//! row only when the list asks for it, so opening a folder of ten thousand files costs one
//! `read_dir` and no per-row work. Names and kinds arrive first (one worker thread), then sizes,
//! dates and item counts are fetched for the rows in view, a batch at a time. Results come back
//! with `slint::invoke_from_event_loop` and find the app again through [`app_do`]; a result that
//! carries an old generation number belongs to a folder that has been left already, and is
//! dropped.
//!
//! A long press selects a row and a tap toggles it while anything is selected; the search field
//! filters the names; the sort sheet and the menu are one modal sheet ([`SheetAction`] says what
//! each of its lines does). All three are the listing's own (`Listing::toggle`, `set_query`,
//! `set_order`), so what this file adds is telling the screen about them.
//!
//! Not here yet (they were in the old UI and come back as slices, see docs/UI-REWRITE.md):
//! the selection bar and the row menus, thumbnails, archives, Recent, Starred and Settings.
//! Their buttons say so in a toast.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};

use kiln_state::fmt::{self as sfmt, Units};
use kiln_state::{Change, Entry, Kind, Listing, Meta, Order, SortKey, scan};
use kiln_ui_next::{BrowseState, CategoryVm, EntryVm, HomeState, Icons, MainWindow, Nav};
use kiln_ui_next::{DialogState, SheetRow, SheetState, ToastState, Tok};
use slint::{
    Color, ComponentHandle, Image, Model, ModelNotify, ModelRc, ModelTracker, SharedString, Timer,
    TimerMode, VecModel,
};

use crate::fmt::io_text;
use crate::host::{Host, OpenWith};
use crate::icons;
use crate::{App, app_do};

/// How many rows of a folder are asked about at once while it is scrolled.
const BATCH: usize = 64;
/// How long a toast stays.
const TOAST_MS: u64 = 2500;
/// A folder that takes longer than this to list says "Loading...".
const LOADING_HINT_MS: u64 = 700;

// ---- turning entries into rows --------------------------------------------------------------

/// The icon paths of `icons.slint` that Rust picks from, read once from the `Icons` global.
#[derive(Clone, Default)]
struct IconSet {
    folder: SharedString,
    file: SharedString,
    image: SharedString,
    music: SharedString,
    play: SharedString,
    document: SharedString,
    archive: SharedString,
    lock: SharedString,
    check: SharedString,
    close: SharedString,
    refresh: SharedString,
}

impl IconSet {
    fn read(ui: &MainWindow) -> Self {
        let i = ui.global::<Icons>();
        Self {
            folder: i.get_folder(),
            file: i.get_file(),
            image: i.get_image(),
            music: i.get_music(),
            play: i.get_play(),
            document: i.get_document(),
            archive: i.get_archive(),
            lock: i.get_lock(),
            check: i.get_check(),
            close: i.get_close(),
            refresh: i.get_refresh(),
        }
    }

    /// The icon of an entry: its kind, then for a file what its extension says.
    fn of(&self, e: &Entry) -> &SharedString {
        match e.kind {
            Kind::Dir | Kind::Set => &self.folder,
            Kind::Archive => &self.archive,
            Kind::File => match icons::for_file(&e.name).0 {
                "image" => &self.image,
                "music" => &self.music,
                "video" => &self.play,
                "f_pdf" | "f_doc" | "f_sheet" | "f_slides" | "f_text" | "f_book" | "code" => {
                    &self.document
                }
                "f_zip" | "f_disk" | "f_apk" => &self.archive,
                "f_key" => &self.lock,
                _ => &self.file,
            },
        }
    }
}

/// What turns an [`Entry`] into the [`EntryVm`] a row draws.
struct Looks {
    host: Rc<dyn Host>,
    icons: IconSet,
    units: Cell<Units>,
    /// Offsets of the phone's time zone from UTC, by day since 1970, so that a folder of
    /// thousands of files asks the phone once per day and not once per file.
    offsets: RefCell<HashMap<i64, i32>>,
}

impl Looks {
    fn offset(&self, epoch: i64) -> i32 {
        *self
            .offsets
            .borrow_mut()
            .entry(epoch.div_euclid(86_400))
            .or_insert_with(|| self.host.utc_offset(epoch))
    }

    /// "23 items • Oct 28, 2026" or "3.41 MB • Nov 6, 2026"; what is not known yet is left out.
    fn meta_line(&self, e: &Entry) -> String {
        let mut parts: Vec<String> = Vec::new();
        if e.kind.is_folder() {
            if let Some(n) = e.children {
                parts.push(sfmt::plural(u64::from(n), "item", "items"));
            }
        } else if let Some(m) = e.meta {
            parts.push(sfmt::size(
                i64::try_from(m.size).unwrap_or(i64::MAX),
                self.units.get(),
            ));
        }
        if let Some(m) = e.meta {
            let date = sfmt::date(m.mtime, self.offset(m.mtime));
            if !date.is_empty() {
                parts.push(date);
            }
        }
        parts.join(" \u{2022} ")
    }

    fn vm(&self, e: &Entry, selected: bool) -> EntryVm {
        let folder = e.kind.is_folder();
        let (stem, ext) = split_name(&e.name, folder);
        EntryVm {
            stem: stem.into(),
            ext: ext.into(),
            meta: self.meta_line(e).into(),
            d: self.icons.of(e).clone(),
            // transparent: the row picks the neutral icon or folder colour of the theme
            tint: Color::from_argb_u8(0, 0, 0, 0),
            selected,
            folder,
            thumb: Image::default(),
            has_thumb: false,
        }
    }
}

/// A name as a row shows it: the stem, which is cut at its end when too long, and the extension
/// (".pdf"), which never is. A folder has no extension, and a "extension" that is long is part
/// of the name.
fn split_name(name: &str, folder: bool) -> (&str, &str) {
    if folder {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 12 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

// ---- the model --------------------------------------------------------------------------------

/// The rows of the folder on screen. Slint asks for a row when it needs to draw it.
pub(crate) struct ListingModel {
    looks: Rc<Looks>,
    listing: RefCell<Listing>,
    notify: ModelNotify,
}

impl ListingModel {
    fn new(looks: Rc<Looks>) -> Self {
        Self {
            looks,
            listing: RefCell::new(Listing::new(Vec::new(), Order::default())),
            notify: ModelNotify::default(),
        }
    }

    fn replace(&self, listing: Listing) {
        *self.listing.borrow_mut() = listing;
        self.notify.reset();
    }

    /// Change the listing, then tell the list which rows that touched.
    fn edit(&self, f: impl FnOnce(&mut Listing) -> Change) {
        let change = f(&mut self.listing.borrow_mut());
        match change {
            Change::None => {}
            Change::Rows(rows) => rows.into_iter().for_each(|r| self.notify.row_changed(r)),
            Change::Reset => self.notify.reset(),
        }
    }
}

impl Model for ListingModel {
    type Data = EntryVm;

    fn row_count(&self) -> usize {
        self.listing.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<EntryVm> {
        let listing = self.listing.borrow();
        let entry = listing.entry(row)?;
        Some(self.looks.vm(entry, listing.is_selected(row)))
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// Two changes of one listing as one.
fn merge(a: Change, b: Change) -> Change {
    match (a, b) {
        (Change::Reset, _) | (_, Change::Reset) => Change::Reset,
        (Change::None, c) | (c, Change::None) => c,
        (Change::Rows(mut x), Change::Rows(y)) => {
            x.extend(y);
            x.sort_unstable();
            x.dedup();
            Change::Rows(x)
        }
    }
}

// ---- sheets -------------------------------------------------------------------------------------

/// What tapping a line of the open sheet does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SheetAction {
    SortKey(SortKey),
    SortDirection(bool),
    SelectAll,
    ClearSelection,
    ShowAll,
    ShowHidden,
    Refresh,
}

/// A line of a sheet and what it does.
type Line = (SheetAction, SheetRow);

fn line(action: SheetAction, label: &str, d: &SharedString, checked: bool) -> Line {
    let row = SheetRow {
        label: label.into(),
        d: d.clone(),
        checked,
    };
    (action, row)
}

/// What a folder that is read again keeps. The selection travels by name, because the entries of
/// the new listing are numbered afresh.
struct Carry {
    selected: HashSet<String>,
}

/// What the confirm button of the open dialog does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DialogAction {
    /// Delete these for good.
    Delete(Vec<PathBuf>),
    /// A message: the button only closes it.
    Close,
}

// ---- the state of the screens -------------------------------------------------------------------

/// A folder that is open, and where its list was scrolled to when a deeper one was opened.
struct Level {
    dir: PathBuf,
    scroll: f32,
}

pub(crate) struct Screens {
    looks: Rc<Looks>,
    /// The top of the phone's own storage.
    root: PathBuf,
    /// The rows of the folder screen.
    model: Rc<ListingModel>,
    /// The folders of the Files tab, and their names (a tap gives the row).
    home: Rc<VecModel<EntryVm>>,
    home_names: RefCell<Vec<String>>,
    /// The folders opened on top of the tabs, the one on screen last.
    levels: RefCell<Vec<Level>>,
    /// Counts the folders opened, so that a late result can tell it is too late.
    generation: Cell<u64>,
    /// The listing of the current folder has arrived.
    loaded: Cell<bool>,
    /// Rows (and whether it is the item count) that a worker thread is looking up.
    pending: RefCell<HashSet<(u32, bool)>>,
    /// The listing could not be read, and the empty line says why.
    failed: Cell<bool>,
    /// What the lines of the open sheet do, line by line (empty when no sheet is open).
    sheet: RefCell<Vec<SheetAction>>,
    /// What the confirm button of the open dialog does (`None` when no dialog is open).
    dialog: RefCell<Option<DialogAction>>,
    /// A file operation is running on a worker thread; a second one is ignored until it is done.
    busy: Cell<bool>,
    /// What the text of the search field says (it is kept across a refresh of the folder).
    query: RefCell<String>,
    /// Set by Refresh, taken by the listing that arrives.
    carry: RefCell<Option<Carry>>,
    loading_hint: Timer,
    toast_timer: Timer,
    /// Whether the files were already asked for once, so that coming back to the app does not
    /// start over.
    started: Cell<bool>,
}

impl Screens {
    pub(crate) fn new(ui: &MainWindow, host: Rc<dyn Host>, units: Units) -> Self {
        let looks = Rc::new(Looks {
            host: host.clone(),
            icons: IconSet::read(ui),
            units: Cell::new(units),
            offsets: RefCell::default(),
        });
        Self {
            model: Rc::new(ListingModel::new(looks.clone())),
            looks,
            root: host.storage_root(),
            home: Rc::new(VecModel::default()),
            home_names: RefCell::default(),
            levels: RefCell::default(),
            generation: Cell::new(0),
            loaded: Cell::new(false),
            pending: RefCell::default(),
            failed: Cell::new(false),
            sheet: RefCell::default(),
            dialog: RefCell::default(),
            busy: Cell::new(false),
            query: RefCell::default(),
            carry: RefCell::default(),
            loading_hint: Timer::default(),
            toast_timer: Timer::default(),
            started: Cell::new(false),
        }
    }
}

impl App {
    // ---- wiring -------------------------------------------------------------------------------

    /// Connect what the screens report to what the app does, and give them their models.
    pub(crate) fn wire_screens(&self) {
        let s = &self.screens;

        let nav = self.ui.global::<Nav>();
        nav.on_back_pressed(|| app_do(|a| a.on_back()));
        let host = self.host.clone();
        nav.on_allow_access(move || host.open_all_files_settings());

        let home = self.ui.global::<HomeState>();
        let i = &s.looks.icons;
        let tile = |label: &str, d: &SharedString| CategoryVm {
            label: label.into(),
            d: d.clone(),
        };
        let categories = [
            tile("Downloads", &self.ui.global::<Icons>().get_download()),
            tile("Images", &i.image),
            tile("Videos", &i.play),
            tile("Audio", &i.music),
            tile("Documents", &i.document),
        ];
        home.set_categories(ModelRc::new(VecModel::from(categories.to_vec())));
        home.set_folders(ModelRc::from(s.home.clone()));
        home.on_folder_clicked(|i| app_do(|a| a.open_home_folder(i)));
        home.on_category_clicked(|i| app_do(|a| a.open_category(i)));
        home.on_storage_clicked(|| app_do(|a| a.open_folder(a.screens.root.clone())));
        home.on_search_clicked(|| app_do(|a| a.soon("Search")));
        home.on_more_clicked(|| app_do(|a| a.soon("This menu")));
        home.on_folder_menu(|_| app_do(|a| a.soon("The folder menu")));

        let browse = self.ui.global::<BrowseState>();
        browse.set_rows(ModelRc::from(s.model.clone()));
        browse.on_back_clicked(|| app_do(|a| a.on_back()));
        browse.on_row_clicked(|i| app_do(|a| a.on_row(i)));
        browse.on_row_long_pressed(|i| app_do(|a| a.on_row_long_pressed(i)));
        browse.on_row_menu(|_| app_do(|a| a.soon("The file menu")));
        browse.on_search_clicked(|| app_do(|a| a.toggle_search()));
        browse.on_query_edited(|q| app_do(|a| a.query_edited(q.as_str())));
        browse.on_sort_clicked(|| app_do(|a| a.sort_sheet()));
        browse.on_more_clicked(|| app_do(|a| a.folder_menu()));
        browse.on_viewed(|| app_do(|a| a.fetch_view()));
        browse.on_bar_clicked(|i| app_do(|a| a.bar_clicked(i)));

        let dialog = self.ui.global::<DialogState>();
        dialog.on_confirmed(|| app_do(|a| a.dialog_confirmed()));
        dialog.on_cancelled(|| {
            app_do(|a| {
                a.close_dialog();
            });
        });

        let sheet = self.ui.global::<SheetState>();
        sheet.on_picked(|i| app_do(|a| a.sheet_picked(i)));
        sheet.on_dismissed(|| {
            app_do(|a| {
                a.close_sheet();
            });
        });
    }

    // ---- toast --------------------------------------------------------------------------------

    /// Show a line over the bottom of the screen for a moment.
    pub(crate) fn toast(&self, text: &str) {
        self.ui.global::<ToastState>().set_text(text.into());
        self.screens.toast_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(TOAST_MS),
            || app_do(|a| a.ui.global::<ToastState>().set_text(SharedString::new())),
        );
    }

    fn soon(&self, what: &str) {
        self.toast(&format!("{what} is not in the new screens yet."));
    }

    // ---- look and permission --------------------------------------------------------------------

    /// Hand the system's colour scheme and font scale, and the user's animation speed, to the
    /// `Tok` global that every component reads.
    pub(crate) fn apply_look(&self) {
        let look = self.host.look();
        let level = self.prefs.int("anim_level", 2).clamp(0, 3) as usize;
        let tok = self.ui.global::<Tok>();
        tok.set_dark(look.dark);
        tok.set_font_scale(look.font_scale);
        tok.set_anim_scale(crate::ANIM_SCALE[level]);
    }

    /// Show or hide the permission screen, and load the Files tab once there is access (and
    /// again whenever the app comes back to the front, because the files may have changed).
    pub(crate) fn refresh_permission(&self) {
        let ok = self.host.has_all_files_access();
        let nav = self.ui.global::<Nav>();
        let was_missing = nav.get_permission_missing();
        nav.set_permission_missing(!ok);
        if !ok {
            return;
        }
        if !self.screens.started.get() || was_missing {
            self.screens.started.set(true);
            self.load_home();
        } else {
            self.refresh_storage();
        }
    }

    // ---- the Files tab ------------------------------------------------------------------------

    fn order(&self) -> Order {
        Order {
            key: SortKey::from_code(self.prefs.int("sort_key", 0)),
            ascending: self.prefs.bool("sort_asc", true),
        }
    }

    /// "78.2 GB / 128 GB" and how full the storage is.
    fn refresh_storage(&self) {
        let home = self.ui.global::<HomeState>();
        let Some((free, total)) = self.host.space(&self.screens.root) else {
            home.set_storage_usage(SharedString::new());
            home.set_storage_fraction(0.0);
            return;
        };
        let used = total.saturating_sub(free);
        let units = self.screens.looks.units.get();
        let bytes = |n: u64| sfmt::size(i64::try_from(n).unwrap_or(i64::MAX), units);
        home.set_storage_usage(format!("{} / {}", bytes(used), bytes(total)).into());
        home.set_storage_fraction(if total == 0 {
            0.0
        } else {
            (used as f64 / total as f64) as f32
        });
    }

    /// The folders at the top of the storage, with their item counts and dates, read off the UI
    /// thread (each of those is a round trip to the phone's storage).
    fn load_home(&self) {
        self.refresh_storage();
        let root = self.screens.root.clone();
        std::thread::spawn(move || {
            let mut entries: Vec<Entry> = scan::list(&root)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.kind.is_folder() && !e.is_hidden())
                .collect();
            for e in &mut entries {
                e.meta = scan::fetch_meta(&root, &e.name);
                e.children = scan::count_children(&root, &e.name);
            }
            let _ = slint::invoke_from_event_loop(move || app_do(|a| a.home_loaded(entries)));
        });
    }

    fn home_loaded(&self, entries: Vec<Entry>) {
        let s = &self.screens;
        let listing = Listing::new(entries, Order::default());
        let mut names = Vec::with_capacity(listing.len());
        let mut rows = Vec::with_capacity(listing.len());
        for row in 0..listing.len() {
            if let Some(e) = listing.entry(row) {
                names.push(e.name.clone());
                rows.push(s.looks.vm(e, false));
            }
        }
        *s.home_names.borrow_mut() = names;
        s.home.set_vec(rows);
    }

    fn open_home_folder(&self, row: i32) {
        let name = usize::try_from(row)
            .ok()
            .and_then(|r| self.screens.home_names.borrow().get(r).cloned());
        if let Some(name) = name {
            self.open_folder(self.screens.root.join(name));
        }
    }

    /// The quick-access tiles open the standard folder of their kind.
    fn open_category(&self, tile: i32) {
        const FOLDERS: [&str; 5] = ["Download", "Pictures", "Movies", "Music", "Documents"];
        let Some(name) = usize::try_from(tile).ok().and_then(|t| FOLDERS.get(t)) else {
            return;
        };
        let dir = self.screens.root.join(name);
        if dir.is_dir() {
            self.open_folder(dir);
        } else {
            self.toast(&format!("This phone has no {name} folder."));
        }
    }

    // ---- going in and out of folders ----------------------------------------------------------

    /// Open `dir` on top of whatever is on screen.
    pub(crate) fn open_folder(&self, dir: PathBuf) {
        let s = &self.screens;
        let browse = self.ui.global::<BrowseState>();
        if let Some(top) = s.levels.borrow_mut().last_mut() {
            top.scroll = browse.get_scroll_y();
        }
        s.levels.borrow_mut().push(Level {
            dir: dir.clone(),
            scroll: 0.0,
        });
        self.ui.global::<Nav>().set_browsing(true);
        self.load_dir(dir, 0.0);
    }

    /// What the Back key and the back button do: close the sheet, leave the selection, leave the
    /// search, go up a folder, then out of the folders, then to the Files tab, then out of the app.
    pub(crate) fn on_back(&self) {
        if self.close_dialog() || self.close_sheet() {
            return;
        }
        let nav = self.ui.global::<Nav>();
        let browse = self.ui.global::<BrowseState>();
        if nav.get_permission_missing() {
            self.host.finish();
        } else if nav.get_browsing() {
            if browse.get_selecting() {
                self.clear_selection();
            } else if browse.get_searching() {
                self.close_search();
            } else {
                self.leave_folder();
            }
        } else if nav.get_tab() != 0 {
            nav.set_tab(0);
        } else {
            self.host.finish();
        }
    }

    fn leave_folder(&self) {
        let s = &self.screens;
        let back_to = {
            let mut levels = s.levels.borrow_mut();
            levels.pop();
            levels.last().map(|l| (l.dir.clone(), l.scroll))
        };
        match back_to {
            Some((dir, scroll)) => self.load_dir(dir, scroll),
            None => {
                // invalidate the folder that is going away, and refresh what the tab shows
                s.generation.set(s.generation.get() + 1);
                self.ui.global::<Nav>().set_browsing(false);
                self.load_home();
            }
        }
    }

    fn on_row(&self, row: i32) {
        if self.ui.global::<BrowseState>().get_selecting() {
            self.toggle_row(row);
            return;
        }
        let s = &self.screens;
        let Some(dir) = s.levels.borrow().last().map(|l| l.dir.clone()) else {
            return;
        };
        let picked = usize::try_from(row).ok().and_then(|r| {
            s.model
                .listing
                .borrow()
                .entry(r)
                .map(|e| (e.name.clone(), e.kind))
        });
        let Some((name, kind)) = picked else {
            return;
        };
        match kind {
            Kind::Dir | Kind::Set => self.open_folder(dir.join(name)),
            Kind::Archive => self.soon("Opening archives"),
            Kind::File => self.open_file(&dir.join(&name), &name),
        }
    }

    /// Hand a file to another app, through the system's "Open with" list.
    fn open_file(&self, path: &Path, name: &str) {
        let mime = self
            .host
            .mime_for_extension(&sfmt::ext(name))
            .unwrap_or_else(|| "*/*".to_string());
        if !self.host.open_file(path, &mime, false, &OpenWith::Chooser) {
            self.toast("No app on this phone can open that file.");
        }
    }

    // ---- reading a folder ---------------------------------------------------------------------

    /// Open `dir` afresh: no selection and no search, the list cleared at once, and its names read
    /// off the UI thread; the list goes to `scroll` when they are there.
    fn load_dir(&self, dir: PathBuf, scroll: f32) {
        let s = &self.screens;
        let browse = self.ui.global::<BrowseState>();
        if browse.get_searching() {
            browse.set_searching(false);
            self.ui.invoke_focus_keys();
        }
        s.query.borrow_mut().clear();
        s.carry.borrow_mut().take();
        browse.set_selecting(false);
        browse.set_selected_count(0);
        s.model.replace(Listing::new(Vec::new(), self.order()));

        let title = if dir == s.root {
            "Internal storage".to_string()
        } else {
            dir.file_name().map_or_else(
                || dir.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        browse.set_title(title.into());
        browse.set_subtitle(dir.display().to_string().into());
        browse.set_empty_text(SharedString::new());
        browse.set_scroll_y(0.0);
        self.read_dir(dir, scroll, true);
    }

    /// Read the names of `dir` off the UI thread. A `fresh` folder says "Loading..." when that
    /// takes long; one that is read again (Refresh) keeps showing its rows until the new ones
    /// arrive.
    fn read_dir(&self, dir: PathBuf, scroll: f32, fresh: bool) {
        let s = &self.screens;
        let generation = s.generation.get() + 1;
        s.generation.set(generation);
        s.loaded.set(false);
        s.failed.set(false);
        s.pending.borrow_mut().clear();

        if fresh {
            s.loading_hint.start(
                TimerMode::SingleShot,
                Duration::from_millis(LOADING_HINT_MS),
                move || {
                    app_do(|a| {
                        let s = &a.screens;
                        if s.generation.get() == generation && !s.loaded.get() {
                            a.ui.global::<BrowseState>()
                                .set_empty_text("Loading...".into());
                        }
                    });
                },
            );
        }

        std::thread::spawn(move || {
            let result = scan::list(&dir).map_err(|e| io_text(&e));
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|a| a.dir_listed(generation, result, scroll));
            });
        });
    }

    fn dir_listed(&self, generation: u64, result: Result<Vec<Entry>, String>, scroll: f32) {
        let s = &self.screens;
        if generation != s.generation.get() {
            return;
        }
        s.loaded.set(true);
        s.loading_hint.stop();
        let browse = self.ui.global::<BrowseState>();
        let carry = s.carry.borrow_mut().take();
        match result {
            Err(text) => {
                s.failed.set(true);
                browse.set_empty_text(text.into());
            }
            Ok(entries) => {
                let mut listing = Listing::new(entries, self.order());
                listing.set_show_hidden(self.prefs.bool("show_hidden", false));
                listing.set_show_all(self.prefs.bool("show_all", true));
                if browse.get_searching() {
                    listing.set_query(&s.query.borrow());
                }
                if let Some(carry) = carry {
                    listing.select_names(&carry.selected);
                }
                let by_meta = listing.order().key.needs_meta();
                s.model.replace(listing);
                self.update_empty_text();
                self.sync_selection();
                // after the rows exist, so that the list does not clamp it to nothing
                browse.set_scroll_y(scroll);
                if by_meta {
                    // sorted by size or date: every row needs its metadata before any is placed
                    self.fetch_rows(0, usize::MAX, usize::MAX);
                }
                self.fetch_view();
            }
        }
    }

    /// The line over the list: why it is empty, or nothing.
    fn update_empty_text(&self) {
        let s = &self.screens;
        if !s.loaded.get() || s.failed.get() {
            return;
        }
        let text = {
            let listing = s.model.listing.borrow();
            if !listing.is_empty() {
                ""
            } else if listing.has_query() {
                "No matches."
            } else if !listing.shows_all() {
                "No Kiln archives here.\nOpen the menu to show all files."
            } else {
                "This folder is empty."
            }
        };
        self.ui.global::<BrowseState>().set_empty_text(text.into());
    }

    // ---- selection ----------------------------------------------------------------------------

    /// Tell the bar how many rows are selected.
    fn sync_selection(&self) {
        let count = self.screens.model.listing.borrow().selected_count();
        let browse = self.ui.global::<BrowseState>();
        browse.set_selecting(count > 0);
        browse.set_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
    }

    /// Flip one row of the list.
    fn toggle_row(&self, row: i32) {
        let s = &self.screens;
        let Ok(row) = usize::try_from(row) else {
            return;
        };
        let in_list = {
            let mut listing = s.model.listing.borrow_mut();
            let in_list = row < listing.len();
            if in_list {
                listing.toggle(row);
            }
            in_list
        };
        if in_list {
            s.model.notify.row_changed(row);
            self.sync_selection();
        }
    }

    /// A long press selects the row; while a folder is still loading it does nothing.
    fn on_row_long_pressed(&self, row: i32) {
        if self.screens.loaded.get() {
            self.toggle_row(row);
        }
    }

    fn select_all(&self) {
        self.screens.model.edit(|l| {
            l.select_all();
            Change::Reset
        });
        self.sync_selection();
    }

    fn clear_selection(&self) {
        self.screens.model.edit(|l| {
            l.clear_selection();
            Change::Reset
        });
        self.sync_selection();
    }

    // ---- search -------------------------------------------------------------------------------

    /// The magnifier opens the search field, and the cross it turns into closes it.
    fn toggle_search(&self) {
        let browse = self.ui.global::<BrowseState>();
        if browse.get_searching() {
            self.close_search();
        } else if self.screens.loaded.get() {
            browse.set_searching(true);
        }
    }

    fn close_search(&self) {
        self.ui.global::<BrowseState>().set_searching(false);
        self.screens.query.borrow_mut().clear();
        self.screens.model.edit(|l| {
            l.set_query("");
            Change::Reset
        });
        // the field had the keyboard focus, and the Back key has to have somewhere to arrive
        self.ui.invoke_focus_keys();
        self.update_empty_text();
        self.fetch_view();
    }

    /// The words of the field filter the names of the folder (every word must occur).
    fn query_edited(&self, text: &str) {
        *self.screens.query.borrow_mut() = text.to_string();
        self.screens.model.edit(|l| {
            l.set_query(text);
            Change::Reset
        });
        self.ui.global::<BrowseState>().set_scroll_y(0.0);
        self.update_empty_text();
        self.fetch_view();
    }

    // ---- the selection bar, dialogs and Delete ----------------------------------------------------

    /// A tile of the selection bar: 0 Copy, 1 Move, 2 Delete, 3 Pack.
    fn bar_clicked(&self, tile: i32) {
        match tile {
            0 => self.soon("Copy"),
            1 => self.soon("Move"),
            2 => self.ask_delete(),
            _ => self.soon("Pack"),
        }
    }

    /// Show the dialog. `cancel` is empty for a message that has only its one button.
    fn open_dialog(
        &self,
        (title, body): (&str, &str),
        (cancel, confirm): (&str, &str),
        danger: bool,
        action: DialogAction,
    ) {
        *self.screens.dialog.borrow_mut() = Some(action);
        let dialog = self.ui.global::<DialogState>();
        dialog.set_title(title.into());
        dialog.set_body(body.into());
        dialog.set_cancel_label(cancel.into());
        dialog.set_confirm_label(confirm.into());
        dialog.set_danger(danger);
        dialog.set_open(true);
    }

    /// A heading, some text and an OK button.
    fn message(&self, title: &str, body: &str) {
        self.open_dialog((title, body), ("", "OK"), false, DialogAction::Close);
    }

    /// Close the dialog; false when none was open.
    fn close_dialog(&self) -> bool {
        let dialog = self.ui.global::<DialogState>();
        let was_open = dialog.get_open();
        if was_open {
            dialog.set_open(false);
            self.screens.dialog.borrow_mut().take();
        }
        was_open
    }

    /// The confirm button was tapped: close the dialog, then do what it asked.
    fn dialog_confirmed(&self) {
        let action = self.screens.dialog.borrow().clone();
        self.close_dialog();
        if let Some(DialogAction::Delete(paths)) = action {
            self.delete_paths(paths);
        }
    }

    /// "Delete <what>?" for the rows that are selected and in view. A selected row that a search
    /// hides is left alone: only what the person can see is asked about, and deleted.
    fn ask_delete(&self) {
        let s = &self.screens;
        if s.busy.get() {
            return;
        }
        let Some(dir) = s.levels.borrow().last().map(|l| l.dir.clone()) else {
            return;
        };
        let (what, paths) = {
            let listing = s.model.listing.borrow();
            let picked = listing.selected_entries();
            let names: Vec<&str> = picked.iter().map(|e| e.name.as_str()).collect();
            let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
            (sfmt::what(&names), paths)
        };
        if paths.is_empty() {
            return;
        }
        self.open_dialog(
            (&format!("Delete {what}?"), "This can't be undone."),
            ("Cancel", "Delete"),
            true,
            DialogAction::Delete(paths),
        );
    }

    /// Delete for good with `fs_delete` on a worker thread. (No progress sheet or Cancel yet.)
    fn delete_paths(&self, paths: Vec<PathBuf>) {
        let s = &self.screens;
        if s.busy.replace(true) {
            return;
        }
        let paths: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        std::thread::spawn(move || {
            let result = kiln_bridge::call("fs_delete", &json!([paths]), 0);
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|a| a.delete_done(&result));
            });
        });
    }

    /// Say what went wrong, if anything, and read the folder again (the selection is gone).
    fn delete_done(&self, result: &Value) {
        self.screens.busy.set(false);
        if result["cancelled"].as_bool().unwrap_or(false) {
            self.toast("Cancelled");
        } else if !result["ok"].as_bool().unwrap_or(false) {
            let why = result["message"].as_str().unwrap_or_default();
            self.message("Couldn't delete everything", why);
        }
        let dir = self.screens.levels.borrow().last().map(|l| l.dir.clone());
        if let Some(dir) = dir {
            let scroll = self.ui.global::<BrowseState>().get_scroll_y();
            self.read_dir(dir, scroll, false);
        }
    }

    // ---- sort, menu and the sheet they share ----------------------------------------------------

    /// Show `lines` in the sheet.
    fn open_sheet(&self, title: &str, lines: Vec<Line>) {
        let (actions, rows): (Vec<_>, Vec<_>) = lines.into_iter().unzip();
        *self.screens.sheet.borrow_mut() = actions;
        let sheet = self.ui.global::<SheetState>();
        sheet.set_title(title.into());
        sheet.set_rows(ModelRc::new(VecModel::from(rows)));
        sheet.set_open(true);
    }

    /// Close the sheet; false when none was open.
    fn close_sheet(&self) -> bool {
        let sheet = self.ui.global::<SheetState>();
        let was_open = sheet.get_open();
        if was_open {
            sheet.set_open(false);
            self.screens.sheet.borrow_mut().clear();
        }
        was_open
    }

    /// "Sort by": the four keys, then the two directions; a choice applies at once.
    fn sort_sheet(&self) {
        let order = self.screens.model.listing.borrow().order();
        let none = SharedString::new();
        let by = |label: &str, key: SortKey| {
            line(SheetAction::SortKey(key), label, &none, order.key == key)
        };
        self.open_sheet(
            "Sort by",
            vec![
                by("Name", SortKey::Name),
                by("Size", SortKey::Size),
                by("Date modified", SortKey::Date),
                by("Type", SortKey::Type),
                line(
                    SheetAction::SortDirection(true),
                    "Ascending",
                    &none,
                    order.ascending,
                ),
                line(
                    SheetAction::SortDirection(false),
                    "Descending",
                    &none,
                    !order.ascending,
                ),
            ],
        );
    }

    /// Keep the order in Settings and show the folder in it.
    fn set_sort(&self, key: SortKey, ascending: bool) {
        self.prefs.set_int("sort_key", key.code());
        self.prefs.set_bool("sort_asc", ascending);
        self.screens.model.edit(|l| {
            l.set_order(Order { key, ascending });
            Change::Reset
        });
        if key.needs_meta() {
            // by size or date every row needs its metadata before any is placed
            self.fetch_rows(0, usize::MAX, usize::MAX);
        }
        self.fetch_view();
    }

    /// The menu of the folder screen: with rows selected, or without.
    fn folder_menu(&self) {
        let selecting = self.screens.model.listing.borrow().is_selecting();
        let i = &self.screens.looks.icons;
        let mut lines = vec![line(SheetAction::SelectAll, "Select all", &i.check, false)];
        if selecting {
            lines.push(line(
                SheetAction::ClearSelection,
                "Clear selection",
                &i.close,
                false,
            ));
        } else {
            let all = self.prefs.bool("show_all", true);
            let hidden = self.prefs.bool("show_hidden", false);
            let all_label = if all {
                "Show archives only"
            } else {
                "Show all files"
            };
            let hidden_label = if hidden {
                "Hide hidden files"
            } else {
                "Show hidden files"
            };
            lines.push(line(SheetAction::ShowAll, all_label, &i.file, false));
            lines.push(line(SheetAction::ShowHidden, hidden_label, &i.file, false));
            lines.push(line(SheetAction::Refresh, "Refresh", &i.refresh, false));
        }
        self.open_sheet("", lines);
    }

    /// A line of the sheet was tapped: close the sheet, then do what the line says.
    fn sheet_picked(&self, line: i32) {
        let action = usize::try_from(line)
            .ok()
            .and_then(|i| self.screens.sheet.borrow().get(i).copied());
        self.close_sheet();
        let Some(action) = action else {
            return;
        };
        let order = self.screens.model.listing.borrow().order();
        match action {
            SheetAction::SortKey(key) => self.set_sort(key, order.ascending),
            SheetAction::SortDirection(ascending) => self.set_sort(order.key, ascending),
            SheetAction::SelectAll => self.select_all(),
            SheetAction::ClearSelection => self.clear_selection(),
            SheetAction::ShowAll => self.toggle_show(false),
            SheetAction::ShowHidden => self.toggle_show(true),
            SheetAction::Refresh => self.refresh_folder(),
        }
    }

    /// "Show all files" or "Show hidden files": flip the setting, keep it, and filter again.
    fn toggle_show(&self, hidden: bool) {
        let (key, default) = if hidden {
            ("show_hidden", false)
        } else {
            ("show_all", true)
        };
        let on = !self.prefs.bool(key, default);
        self.prefs.set_bool(key, on);
        self.screens.model.edit(|l| {
            if hidden {
                l.set_show_hidden(on);
            } else {
                l.set_show_all(on);
            }
            Change::Reset
        });
        self.update_empty_text();
        self.fetch_view();
    }

    /// Read the folder again, keeping the selection, the search and the place in the list.
    fn refresh_folder(&self) {
        let s = &self.screens;
        let Some(dir) = s.levels.borrow().last().map(|l| l.dir.clone()) else {
            return;
        };
        let scroll = self.ui.global::<BrowseState>().get_scroll_y();
        let selected = s.model.listing.borrow().selected_names();
        *s.carry.borrow_mut() = Some(Carry { selected });
        self.read_dir(dir, scroll, false);
    }

    // ---- sizes, dates and counts ----------------------------------------------------------------

    /// Fetch what the rows in view (and a few more each way) are still missing.
    pub(crate) fn fetch_view(&self) {
        let browse = self.ui.global::<BrowseState>();
        let first = usize::try_from(browse.get_view_first()).unwrap_or(0);
        let count = usize::try_from(browse.get_view_count()).unwrap_or(0);
        self.fetch_rows(first.saturating_sub(4), count + 8, BATCH);
    }

    /// Look up the metadata and item counts missing from `count` rows from `first`, on a worker
    /// thread, and send the results back `batch` rows at a time.
    fn fetch_rows(&self, first: usize, count: usize, batch: usize) {
        let s = &self.screens;
        let Some(dir) = s.levels.borrow().last().map(|l| l.dir.clone()) else {
            return;
        };
        let (metas, counts) = {
            let listing = s.model.listing.borrow();
            let mut pending = s.pending.borrow_mut();
            let name = |id: u32| listing.entry_by_id(id).map(|e| e.name.clone());
            let metas: Vec<(u32, String)> = listing
                .missing_meta(first, count)
                .into_iter()
                .filter(|&id| pending.insert((id, false)))
                .filter_map(|id| name(id).map(|n| (id, n)))
                .collect();
            let counts: Vec<(u32, String)> = listing
                .missing_counts(first, count)
                .into_iter()
                .filter(|&id| pending.insert((id, true)))
                .filter_map(|id| name(id).map(|n| (id, n)))
                .collect();
            (metas, counts)
        };
        if metas.is_empty() && counts.is_empty() {
            return;
        }
        let generation = s.generation.get();
        std::thread::spawn(move || {
            let send = |metas: Vec<(u32, Meta)>, counts: Vec<(u32, u32)>| {
                let _ = slint::invoke_from_event_loop(move || {
                    app_do(|a| a.meta_arrived(generation, &metas, &counts));
                });
            };
            for chunk in metas.chunks(batch.max(1)) {
                // a file that cannot be read gets zeros, so that it is not asked for again
                let got = chunk
                    .iter()
                    .map(|(id, name)| (*id, scan::fetch_meta(&dir, name).unwrap_or_default()))
                    .collect();
                send(got, Vec::new());
            }
            for chunk in counts.chunks(batch.max(1)) {
                let got = chunk
                    .iter()
                    .map(|(id, name)| (*id, scan::count_children(&dir, name).unwrap_or(0)))
                    .collect();
                send(Vec::new(), got);
            }
        });
    }

    fn meta_arrived(&self, generation: u64, metas: &[(u32, Meta)], counts: &[(u32, u32)]) {
        let s = &self.screens;
        if generation != s.generation.get() {
            return;
        }
        {
            let mut pending = s.pending.borrow_mut();
            metas.iter().for_each(|(id, _)| {
                pending.remove(&(*id, false));
            });
            counts.iter().for_each(|(id, _)| {
                pending.remove(&(*id, true));
            });
        }
        s.model
            .edit(|l| merge(l.apply_meta(metas), l.apply_counts(counts)));
    }
}
