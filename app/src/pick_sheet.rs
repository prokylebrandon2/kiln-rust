//! The folder icon picker: "Icon and color..." in the selection menu. A colour swatch is saved
//! and the sheet stays up; an icon is saved and closes it; "Use default" clears both; Cancel, Back
//! and a tap outside change nothing.
//!
//! What the sheet shows is worked out by [`plan`], which touches no window, so it is tested;
//! [`Marks`] is what the rows read (the icons and colours that were picked, held in memory so that
//! drawing a row never parses a setting); [`App::open_pick`] shows the plan.

use std::collections::HashMap;
use std::path::PathBuf;

use kiln_ui_next::{PickCell, PickState};
use slint::{Color, ComponentHandle, ModelRc, VecModel};

use crate::icons;
use crate::prefs::Prefs;
use crate::{App, app_do};

/// The colours a folder can be given. The first swatch, "default", is the accent colour and
/// saves 0.
pub(crate) const FOLDER_COLORS: [u32; 10] = [
    0xFFE5574B, 0xFFE8833A, 0xFFE0A030, 0xFF5DB75D, 0xFF3FA6A0, 0xFF4A8FE7, 0xFF5B6EEF, 0xFF8E7CC3,
    0xFFC45BA3, 0xFF8D99AE,
];

/// The icons the picker offers, in the Java's `Icons.FOLDER_KEYS` order: the key and its label.
pub(crate) const PICK_ICONS: [(&str, &str); 18] = [
    ("folder", "Folder"),
    ("download", "Download"),
    ("camera", "Camera"),
    ("pictures", "Pictures"),
    ("movies", "Movies"),
    ("music", "Music"),
    ("documents", "Documents"),
    ("android", "Android"),
    ("bell", "Sounds"),
    ("zip", "Archives"),
    ("star", "Favourite"),
    ("heart", "Heart"),
    ("lock", "Private"),
    ("code", "Code"),
    ("home", "Home"),
    ("phone", "Phone storage"),
    ("sd", "SD card"),
    ("usb", "USB drive"),
];

/// A colour saved as an ARGB number.
pub(crate) fn argb(color: u32) -> Color {
    let [a, r, g, b] = color.to_be_bytes();
    Color::from_argb_u8(a, r, g, b)
}

/// Which swatch shows the tick for a saved colour: 0 is "default", -1 is none of them.
pub(crate) fn color_index(color: u32) -> i32 {
    if color == 0 {
        return 0;
    }
    match FOLDER_COLORS.iter().position(|&c| c == color) {
        Some(i) => i as i32 + 1,
        None => -1,
    }
}

/// The colour a swatch saves: 0 for "default", else one of [`FOLDER_COLORS`].
fn swatch_color(index: i32) -> Option<u32> {
    let i = usize::try_from(index).ok()?;
    match i {
        0 => Some(0),
        _ => FOLDER_COLORS.get(i - 1).copied(),
    }
}

// ---- what the rows read ---------------------------------------------------------------------------

/// The icons and colours that were picked for folders (path to key, path to ARGB).
#[derive(Default)]
pub(crate) struct Marks {
    icons: HashMap<String, String>,
    colors: HashMap<String, u32>,
    /// The colour every folder gets unless it has its own (0: none).
    default_color: u32,
}

impl Marks {
    pub(crate) fn load(prefs: &Prefs) -> Self {
        let icons = prefs
            .folder_icon_map()
            .into_iter()
            .filter_map(|(path, key)| Some((path, key.as_str()?.to_string())))
            .collect();
        let colors = prefs
            .folder_color_map()
            .into_iter()
            .filter_map(|(path, value)| {
                let color = value.as_i64()? as u32;
                (color != 0).then_some((path, color))
            })
            .collect();
        Self {
            icons,
            colors,
            default_color: prefs.default_folder_color(),
        }
    }

    /// The icon key of a folder: the one picked for it, else the one Android's standard folders
    /// have (`parent` is its parent's name; `at_root` says it sits in the top of the storage),
    /// else none (the plain folder).
    pub(crate) fn icon_key<'a>(
        &'a self,
        path: &str,
        name: &str,
        parent: &str,
        at_root: bool,
    ) -> Option<&'a str> {
        match self.icons.get(path) {
            Some(key) => Some(key.as_str()),
            None => icons::default_key(name, parent, at_root),
        }
    }

    /// A folder's colour: its own, else the default, else 0 (the folder colour of the theme).
    pub(crate) fn color(&self, path: &str) -> u32 {
        self.colors.get(path).copied().unwrap_or(self.default_color)
    }
}

// ---- the plan ---------------------------------------------------------------------------------------

/// What the sheet shows when it opens.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    title: String,
    has_colors: bool,
    /// The swatch with the tick (see [`color_index`]); -1 when there are no swatches.
    color_index: i32,
    /// What the icons are drawn in: the folder's colour, else the accent (`None`).
    tint: Option<u32>,
    /// Key, label and whether it is the icon the folder has now.
    cells: Vec<(&'static str, &'static str, bool)>,
}

/// The sheet for the folder called `name`. `color` is the folder's colour (0: none) and `picked`
/// the icon key that was picked for it, if any; a folder that has none shows no icon as current,
/// as in the Java.
fn plan(name: &str, with_colors: bool, picked: Option<&str>, color: u32) -> Plan {
    let heading = if with_colors {
        "Icon and color for "
    } else {
        "Icon for "
    };
    Plan {
        title: format!("{heading}{name}"),
        has_colors: with_colors,
        color_index: if with_colors { color_index(color) } else { -1 },
        tint: (with_colors && color != 0).then_some(color),
        cells: PICK_ICONS
            .iter()
            .map(|&(key, label)| (key, label, picked == Some(key)))
            .collect(),
    }
}

/// The picker that is up: the folder it is about, and whether it has swatches.
#[derive(Default)]
pub(crate) struct Open {
    path: Option<PathBuf>,
    with_colors: bool,
}

impl App {
    pub(crate) fn wire_pick(&self) {
        let state = self.ui.global::<PickState>();
        state.on_swatch_clicked(|i| app_do(|a| a.pick_swatch(i)));
        state.on_icon_clicked(|i| app_do(|a| a.pick_icon(i)));
        state.on_default_clicked(|| app_do(|a| a.pick_default()));
        state.on_cancelled(|| {
            app_do(|a| {
                a.close_pick();
            });
        });
    }

    /// Open the sheet for the folder at `path`, called `name`. Without `with_colors` there are no
    /// swatches (the picker for a bookmark).
    pub(crate) fn open_pick(&self, path: PathBuf, name: &str, with_colors: bool) {
        let key = path.to_string_lossy().into_owned();
        let picked = self.prefs.folder_icon(&key);
        let color = self.prefs.folder_color_or_default(&key);
        let plan = plan(name, with_colors, picked.as_deref(), color);

        let state = self.ui.global::<PickState>();
        state.set_title(plan.title.into());
        state.set_has_colors(plan.has_colors);
        let swatches: Vec<Color> = FOLDER_COLORS.iter().map(|&c| argb(c)).collect();
        state.set_colors(ModelRc::new(VecModel::from(swatches)));
        let i = self.icon_paths();
        let cells: Vec<PickCell> = plan
            .cells
            .iter()
            .map(|&(key, label, current)| PickCell {
                d: i.dir(key).clone(),
                label: label.into(),
                current,
            })
            .collect();
        state.set_cells(ModelRc::new(VecModel::from(cells)));
        self.show_pick_color(plan.color_index, plan.tint);

        *self.pick.borrow_mut() = Open {
            path: Some(path),
            with_colors,
        };
        state.set_open(true);
    }

    /// Move the tick and re-tint the icons.
    fn show_pick_color(&self, index: i32, tint: Option<u32>) {
        let state = self.ui.global::<PickState>();
        state.set_color_index(index);
        let accent = self.ui.global::<kiln_ui_next::Tok>().get_accent();
        state.set_tint(tint.map_or(accent, argb));
    }

    /// A swatch was tapped: save the colour, move the tick, re-tint, update the list behind. The
    /// sheet stays open.
    fn pick_swatch(&self, index: i32) {
        let Some(color) = swatch_color(index) else {
            return;
        };
        let Some(path) = self.pick.borrow().path.clone() else {
            return;
        };
        let key = path.to_string_lossy().into_owned();
        self.prefs.set_folder_color(&key, color);
        let now = self.prefs.folder_color_or_default(&key);
        self.show_pick_color(color_index(now), (now != 0).then_some(now));
        self.marks_changed(&path);
    }

    /// An icon was tapped: save its key and close the sheet.
    fn pick_icon(&self, index: i32) {
        let Some(&(key, _)) = usize::try_from(index).ok().and_then(|i| PICK_ICONS.get(i)) else {
            return;
        };
        let Some(path) = self.pick.borrow().path.clone() else {
            return;
        };
        self.prefs.set_folder_icon(&path.to_string_lossy(), key);
        self.close_pick();
        self.marks_changed(&path);
    }

    /// "Use default": clear the icon, and the colour when the swatches are shown.
    fn pick_default(&self) {
        let (path, with_colors) = {
            let open = self.pick.borrow();
            (open.path.clone(), open.with_colors)
        };
        let Some(path) = path else {
            return;
        };
        let key = path.to_string_lossy().into_owned();
        self.prefs.set_folder_icon(&key, "");
        if with_colors {
            self.prefs.set_folder_color(&key, 0);
        }
        self.close_pick();
        self.marks_changed(&path);
    }

    /// Close the sheet; false when none was open.
    pub(crate) fn close_pick(&self) -> bool {
        let state = self.ui.global::<PickState>();
        let was_open = state.get_open();
        if was_open {
            state.set_open(false);
        }
        *self.pick.borrow_mut() = Open::default();
        was_open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_swatches_are_default_then_ten_colours() {
        assert_eq!(swatch_color(0), Some(0));
        assert_eq!(swatch_color(1), Some(0xFFE5574B));
        assert_eq!(swatch_color(10), Some(0xFF8D99AE));
        assert_eq!(swatch_color(11), None);
        assert_eq!(swatch_color(-1), None);
    }

    #[test]
    fn the_tick_follows_the_saved_colour() {
        assert_eq!(color_index(0), 0);
        assert_eq!(color_index(0xFFE5574B), 1);
        assert_eq!(color_index(0xFF4A8FE7), 6);
        assert_eq!(color_index(0xFF8D99AE), 10);
        // a colour that is not in the list ticks nothing
        assert_eq!(color_index(0xFF123456), -1);
        // every swatch ticks itself
        for i in 0..=10 {
            assert_eq!(color_index(swatch_color(i).unwrap()), i);
        }
    }

    #[test]
    fn argb_keeps_the_alpha() {
        let c = argb(0xFF4A8FE7);
        assert_eq!(
            (c.alpha(), c.red(), c.green(), c.blue()),
            (0xFF, 0x4A, 0x8F, 0xE7)
        );
    }

    #[test]
    fn a_folder_gets_swatches_a_bookmark_does_not() {
        let folder = plan("Music", true, None, 0xFF4A8FE7);
        assert_eq!(folder.title, "Icon and color for Music");
        assert!(folder.has_colors);
        assert_eq!(folder.color_index, 6);
        assert_eq!(folder.tint, Some(0xFF4A8FE7));

        let bookmark = plan("Music", false, None, 0xFF4A8FE7);
        assert_eq!(bookmark.title, "Icon for Music");
        assert!(!bookmark.has_colors);
        assert_eq!(bookmark.color_index, -1);
        assert_eq!(bookmark.tint, None);
    }

    #[test]
    fn no_colour_means_the_default_swatch_and_the_accent() {
        let p = plan("Docs", true, None, 0);
        assert_eq!(p.color_index, 0);
        assert_eq!(p.tint, None);
    }

    #[test]
    fn the_grid_has_eighteen_icons_and_marks_only_the_picked_one() {
        let p = plan("Docs", true, Some("heart"), 0);
        assert_eq!(p.cells.len(), 18);
        assert_eq!(p.cells[0], ("folder", "Folder", false));
        assert_eq!(p.cells[11], ("heart", "Heart", true));
        assert_eq!(p.cells.iter().filter(|c| c.2).count(), 1);
        // nothing picked, or a key this version does not know: nothing is marked
        assert!(plan("Docs", true, None, 0).cells.iter().all(|c| !c.2));
        assert!(
            plan("Docs", true, Some("nope"), 0)
                .cells
                .iter()
                .all(|c| !c.2)
        );
    }

    #[test]
    fn every_icon_of_the_grid_has_a_picture() {
        // `by_key` falls back to the plain folder for a key it does not know, so a key that is
        // missing there would show as a second "Folder"
        for &(key, _) in PICK_ICONS.iter().skip(1) {
            assert_ne!(icons::by_key(key), "folder", "{key}");
        }
    }

    fn marks(xml: &str) -> Marks {
        Marks::load(&Prefs::parse(xml))
    }

    #[test]
    fn marks_prefer_the_picked_icon_to_the_standard_one() {
        let m = marks(
            r#"<map><string name="folder_icons">{"/s/Download":"heart","/s/Misc":"code"}</string></map>"#,
        );
        assert_eq!(
            m.icon_key("/s/Download", "Download", "s", true),
            Some("heart")
        );
        assert_eq!(m.icon_key("/s/Misc", "Misc", "s", true), Some("code"));
        // nothing picked: the standard icon of Android's own folders, else the plain folder
        assert_eq!(m.icon_key("/s/Music", "Music", "s", true), Some("music"));
        assert_eq!(m.icon_key("/s/Music", "Music", "s", false), None);
        assert_eq!(m.icon_key("/s/Other", "Other", "s", true), None);
    }

    #[test]
    fn marks_colour_is_the_folders_own_then_the_default() {
        // Android stores an ARGB colour as a signed number
        let own = 0xFF4A8FE7_u32 as i32;
        let xml = format!(
            r#"<map><string name="folder_colors">{{"/s/A":{own}}}</string><int name="folder_color_default" value="{}" /></map>"#,
            0xFFE5574B_u32 as i32
        );
        let m = marks(&xml);
        assert_eq!(m.color("/s/A"), 0xFF4A8FE7);
        assert_eq!(m.color("/s/B"), 0xFFE5574B);
        // no default: the theme's folder colour
        assert_eq!(marks("<map></map>").color("/s/B"), 0);
    }
}
