//! The facts the Properties box shows (`Props.java`): sizes, dates, permissions, the mount a file
//! lives on, and the totals of a folder or a selection. No screen code lives here.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::fmt;

/// A walk reports at most this often while it counts (`Props.walk`).
const REPORT_EVERY: Duration = Duration::from_millis(150);

/// `Props.sizeText`: "512 B", or "1.50 MB (1572864 B)".
pub fn size_text(n: i64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{} ({n} B)", fmt::size(n))
    }
}

/// `File.lastModified`, in epoch seconds (0 when the file cannot be read).
pub fn modified(path: &Path) -> i64 {
    std::fs::metadata(path).map_or(0, |m| m.mtime())
}

/// "yyyy-MM-dd HH:mm:ss" in the zone whose offset from UTC is `utc_offset` seconds.
pub fn modified_text(epoch_seconds: i64, utc_offset: i32) -> String {
    let date = fmt::date(epoch_seconds, utc_offset);
    if date.is_empty() {
        return date;
    }
    let second = (epoch_seconds + i64::from(utc_offset)).rem_euclid(60);
    format!("{date}:{second:02}")
}

/// Three permission bits as "rw-".
fn triple(bits: u32) -> String {
    [(4, 'r'), (2, 'w'), (1, 'x')]
        .iter()
        .map(|&(bit, letter)| if bits & bit != 0 { letter } else { '-' })
        .collect()
}

/// `Props.permissions`: "rw- rw- --- (660)", or `None` when the file cannot be read.
pub fn permissions(path: &Path) -> Option<String> {
    let mode = std::fs::metadata(path).ok()?.mode() & 0o777;
    let [user, group, other] = [6, 3, 0].map(|shift| triple((mode >> shift) & 7));
    Some(format!("{user} {group} {other} ({mode:o})"))
}

/// `Props.mountOf`: the mount point that holds `path` and its file system type, read from
/// `/proc/mounts`. The longest mount point that contains the path wins, the later one on a tie.
pub fn mount_of(path: &Path) -> Option<(String, String)> {
    let path = std::fs::canonicalize(path)
        .ok()?
        .to_string_lossy()
        .into_owned();
    let table = std::fs::read("/proc/mounts").ok()?;
    let mut best: Option<(String, String)> = None;
    for line in String::from_utf8_lossy(&table).lines() {
        let parts: Vec<&str> = line.split(' ').collect();
        let [_, mount, kind, ..] = parts.as_slice() else {
            continue;
        };
        let mount = mount.replace("\\040", " ");
        let inside = mount == "/" || path == mount || path.starts_with(&format!("{mount}/"));
        let longer = best.as_ref().is_none_or(|(have, _)| mount.len() >= have.len());
        if inside && longer {
            best = Some((mount, (*kind).to_string()));
        }
    }
    best
}

/// What a walk has counted so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Totals {
    pub files: u64,
    pub bytes: u64,
    pub folders: u64,
}

struct Walk<'a> {
    totals: Totals,
    last: Option<Instant>,
    stop: &'a AtomicBool,
    folders_too: bool,
    report: &'a mut dyn FnMut(Totals, bool),
}

impl Walk<'_> {
    /// `Props.walk`: count one entry and what is below it. False when `stop` was set meanwhile.
    fn visit(&mut self, path: &Path, top: bool) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        if is_dir {
            if !top || self.folders_too {
                self.totals.folders += 1;
            }
            if let Ok(kids) = std::fs::read_dir(path) {
                for kid in kids.flatten() {
                    if !self.visit(&kid.path(), false) {
                        return false;
                    }
                }
            }
        } else {
            self.totals.files += 1;
            self.totals.bytes += std::fs::metadata(path).map_or(0, |m| m.len());
        }
        if self.last.is_none_or(|at| at.elapsed() > REPORT_EVERY) {
            self.last = Some(Instant::now());
            (self.report)(self.totals, true);
        }
        true
    }
}

/// `Props.totals`: walk the roots (links are counted as files, not followed) and call `report`
/// with the counts so far and `true` about every 150 ms, then once with the final counts and
/// `false`. `folders_too` counts the roots that are folders as well (a selection of several, not
/// a single folder). Returns false, without a final report, when `stop` was set.
pub fn totals(
    roots: &[&Path],
    folders_too: bool,
    stop: &AtomicBool,
    report: &mut dyn FnMut(Totals, bool),
) -> bool {
    let mut walk = Walk {
        totals: Totals::default(),
        last: None,
        stop,
        folders_too,
        report,
    };
    for root in roots {
        if !walk.visit(root, true) {
            return false;
        }
    }
    (walk.report)(walk.totals, false);
    true
}