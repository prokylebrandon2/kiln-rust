//! The Explorer menu: a cascading "Kiln" entry made of plain registry verbs under
//! `HKEY_CURRENT_USER\Software\Classes` (so it needs no administrator and no DLL in Explorer).
//!
//! * `.zip` and `.kiln` files: Extract Here, Extract to subfolder, Test archive.
//! * every other file, and folders: Add to .kiln, Add to .zip.
//!
//! The entry has one fixed label for every file, and a verb is hidden when several items are
//! selected (`MultiSelectModel = Single`): showing a file's own name in the label (`Add to
//! "name.zip"`) and acting on a whole selection both need a shell extension DLL.

use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegSetValueExW,
};

use crate::w::wide;

struct Item {
    /// Registry key name; Explorer lists the entries in this order.
    id: &'static str,
    label: &'static str,
    /// The first argument of `kiln-gui` (see `main.rs`).
    verb: &'static str,
}

struct Menu {
    /// Under `Software\Classes`.
    root: &'static str,
    key: &'static str,
    items: &'static [Item],
    /// An Advanced Query Syntax filter, to keep the entry off some files.
    applies_to: Option<&'static str>,
}

const ARCHIVE_ITEMS: &[Item] = &[
    Item {
        id: "10extract-here",
        label: "Extract Here",
        verb: "x",
    },
    Item {
        id: "20extract-to",
        label: "Extract to subfolder",
        verb: "xf",
    },
    Item {
        id: "30test",
        label: "Test archive",
        verb: "t",
    },
];

const ADD_ITEMS: &[Item] = &[
    Item {
        id: "10add-kiln",
        label: "Add to .kiln",
        verb: "ak",
    },
    Item {
        id: "20add-zip",
        label: "Add to .zip",
        verb: "az",
    },
];

const NOT_AN_ARCHIVE: &str =
    r#"NOT (System.FileExtension:=".zip" OR System.FileExtension:=".kiln")"#;

const MENUS: &[Menu] = &[
    Menu {
        root: "*",
        key: "KilnAdd",
        items: ADD_ITEMS,
        applies_to: Some(NOT_AN_ARCHIVE),
    },
    Menu {
        root: "Directory",
        key: "KilnAdd",
        items: ADD_ITEMS,
        applies_to: None,
    },
    Menu {
        root: r"SystemFileAssociations\.zip",
        key: "Kiln",
        items: ARCHIVE_ITEMS,
        applies_to: None,
    },
    Menu {
        root: r"SystemFileAssociations\.kiln",
        key: "Kiln",
        items: ARCHIVE_ITEMS,
        applies_to: None,
    },
];

fn base(menu: &Menu) -> String {
    format!(r"Software\Classes\{}\shell\{}", menu.root, menu.key)
}

/// An open registry key, closed again on drop.
struct Key(HKEY);

impl Key {
    fn create(path: &str) -> Result<Key, String> {
        let name = wide(path);
        let mut key: HKEY = null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                0,
                null(),
                0,
                KEY_WRITE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if status == 0 {
            Ok(Key(key))
        } else {
            Err(format!(
                "Windows refused to create {path} (error {status})."
            ))
        }
    }

    /// Set a text value; an empty `name` is the key's default value.
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let name_w = wide(name);
        let value_w = wide(value);
        let bytes = (value_w.len() * 2) as u32;
        let status = unsafe {
            RegSetValueExW(
                self.0,
                name_w.as_ptr(),
                0,
                REG_SZ,
                value_w.as_ptr().cast::<u8>(),
                bytes,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("Windows refused to set {name} (error {status})."))
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// Add the menu, pointing at `exe`. Whatever an earlier `register` left is replaced.
pub fn register(exe: &Path) -> Result<(), String> {
    unregister()?;
    let exe = exe.display().to_string();
    for menu in MENUS {
        let base = base(menu);
        let key = Key::create(&base)?;
        key.set("MUIVerb", "Kiln")?;
        key.set("SubCommands", "")?;
        key.set("MultiSelectModel", "Single")?;
        if let Some(filter) = menu.applies_to {
            key.set("AppliesTo", filter)?;
        }
        for item in menu.items {
            let item_path = format!(r"{base}\shell\{}", item.id);
            Key::create(&item_path)?.set("MUIVerb", item.label)?;
            let command = format!("\"{exe}\" {} \"%1\"", item.verb);
            Key::create(&format!(r"{item_path}\command"))?.set("", &command)?;
        }
    }
    Ok(())
}

/// Remove the menu (nothing to remove is fine).
pub fn unregister() -> Result<(), String> {
    for menu in MENUS {
        let path = wide(&base(menu));
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
        // 0 = removed, 2 = there was nothing to remove.
        if status != 0 && status != 2 {
            return Err(format!(
                "Windows refused to remove the menu (error {status})."
            ));
        }
    }
    Ok(())
}
