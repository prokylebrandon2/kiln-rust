//! Thumbnail previews without any view code: which files get one, the size to
//! decode at, a bounded cache of the results and the two worker threads that decode them.
//!
//! The phone does the decoding ([`ThumbDecoder`], implemented in the platform crate with
//! And `MediaMetadataRetriever`); everything else here runs anywhere, so it is
//! tested here. `thumbs_ui.rs` puts the results on screen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

/// A picture bigger than this (in bytes) gets no preview.
pub const IMAGE_MAX: i64 = 15_000_000;
/// The size a preview is decoded at, in dp (`Ui.dp(c, 96)`).
const PREVIEW_DP: f32 = 96.0;
/// A preview never needs more pixels than this; a very wide picture is decoded smaller still.
const MAX_PIXELS: i64 = 1_000_000;

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "heif"];
const AUDIO_EXT: &[&str] = &[
    "mp3", "m4a", "flac", "ogg", "oga", "opus", "wav", "aac", "wma", "mka", "amr",
];
const VIDEO_EXT: &[&str] = &["mp4", "mkv", "webm", "3gp", "mov", "avi", "m4v", "ts"];

/// The kinds of file that get a preview: code and other files only get icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbKind {
    /// The picture itself.
    Image,
    /// The cover art inside the file.
    Audio,
    /// A frame one second in.
    Video,
}

/// For the kinds that can be previewed. `ext` is the extension without the dot.
pub fn kind_of(ext: &str) -> Option<ThumbKind> {
    let is = |list: &[&str]| list.iter().any(|e| e.eq_ignore_ascii_case(ext));
    if is(IMAGE_EXT) {
        Some(ThumbKind::Image)
    } else if is(AUDIO_EXT) {
        Some(ThumbKind::Audio)
    } else if is(VIDEO_EXT) {
        Some(ThumbKind::Video)
    } else {
        None
    }
}

/// Whether a file with this extension and size (in bytes) is previewed.
pub fn eligible(ext: &str, size: i64) -> Option<ThumbKind> {
    let kind = kind_of(ext)?;
    (kind != ThumbKind::Image || size <= IMAGE_MAX).then_some(kind)
}

/// The size to decode at, in pixels, on a screen of this density.
pub fn px_for(density: f32) -> u32 {
    (PREVIEW_DP * density).round().max(1.0) as u32
}

/// For a picture of `width` x `height`: halve while both sides stay at least `px`
/// and further while the result would still be over [`MAX_PIXELS`].
pub fn sample_size(width: i32, height: i32, px: i32) -> i32 {
    let px = px.max(1);
    let mut s: i32 = 1;
    while s < 1 << 20 && width / (s * 2) >= px && height / (s * 2) >= px {
        s *= 2;
    }
    while s < 1 << 20 && i64::from(width / s) * i64::from(height / s) > MAX_PIXELS {
        s *= 2;
    }
    s
}

/// A decoded preview: `width` x `height` pixels, four bytes each (R, G, B, A; not premultiplied).
#[derive(Clone, Debug)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the phone does for the previews. It is called on the worker threads.
pub trait ThumbDecoder: Send + Sync {
    /// Decode a preview of `path` at about `px` pixels, or `None` when there is nothing to show
    /// (not a picture, no cover art, a file that cannot be read).
    fn decode(&self, path: &Path, kind: ThumbKind, px: u32) -> Option<Pixels>;
}

/// What a preview belongs to. The time and size are the ones the folder scan saw, so a file that
/// changed is a different key, and so is another decode size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub path: PathBuf,
    pub time: i64,
    pub size: i64,
    pub px: u32,
}

struct Slot<V> {
    value: V,
    cost: usize,
    used: u64,
}

/// A cache that holds at most `max_bytes` (by the cost given with each value) and `max_entries`
/// values, and lets go of the one used longest ago first. Failed decodes are cached too.
pub struct Cache<V> {
    map: HashMap<Key, Slot<V>>,
    bytes: usize,
    max_bytes: usize,
    max_entries: usize,
    tick: u64,
}

impl<V: Clone> Cache<V> {
    pub fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            max_bytes,
            max_entries,
            tick: 0,
        }
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.map.contains_key(key)
    }

    /// Count the key as just used. False when it is not there.
    pub fn touch(&mut self, key: &Key) -> bool {
        self.tick += 1;
        let tick = self.tick;
        match self.map.get_mut(key) {
            Some(slot) => {
                slot.used = tick;
                true
            }
            None => false,
        }
    }

    /// The value, counted as just used.
    pub fn get(&mut self, key: &Key) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|slot| {
            slot.used = tick;
            slot.value.clone()
        })
    }

    /// Store a value. Returns the keys that had to go to make room, or `None` when the value is
    /// too big for the cache and was not stored.
    pub fn put(&mut self, key: Key, value: V, cost: usize) -> Option<Vec<Key>> {
        if cost > self.max_bytes {
            return None;
        }
        self.tick += 1;
        let slot = Slot {
            value,
            cost,
            used: self.tick,
        };
        if let Some(old) = self.map.insert(key, slot) {
            self.bytes -= old.cost;
        }
        self.bytes += cost;
        let mut evicted = Vec::new();
        while self.bytes > self.max_bytes || self.map.len() > self.max_entries {
            let oldest = self
                .map
                .iter()
                .min_by_key(|(_, slot)| slot.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            if let Some(gone) = self.map.remove(&oldest) {
                self.bytes -= gone.cost;
            }
            evicted.push(oldest);
        }
        Some(evicted)
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// One preview to decode.
#[derive(Clone, Debug)]
pub struct Job {
    pub key: Key,
    pub kind: ThumbKind,
}

/// What the workers call with each result (on the worker's thread).
pub type Done = Arc<dyn Fn(Key, Option<Pixels>) + Send + Sync>;

/// `Executors.newFixedThreadPool(2)`.
const WORKERS: usize = 2;

struct Queue {
    pending: VecDeque<Job>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

/// The worker threads and the jobs waiting for them. Dropping the pool stops the workers once
/// their current job is done.
pub struct Pool {
    shared: Arc<Shared>,
}

impl Pool {
    fn idle() -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue {
                    pending: VecDeque::new(),
                    closed: false,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    /// Start the workers. `done` is called for every job that was run, with `None` when there is
    /// no preview to show.
    pub fn start(decoder: Arc<dyn ThumbDecoder>, done: Done) -> Self {
        let pool = Self::idle();
        for n in 0..WORKERS {
            let shared = pool.shared.clone();
            let decoder = decoder.clone();
            let done = done.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("thumbs-{n}"))
                .spawn(move || worker(&shared, decoder.as_ref(), done.as_ref()));
            let _ = spawned;
        }
        pool
    }

    /// Say what is wanted now: the jobs that have not started and whose key is not in `visible`
    /// are dropped, and
    /// `fresh` is queued. Returns the keys that were dropped.
    pub fn retarget(&self, visible: &HashSet<Key>, fresh: Vec<Job>) -> Vec<Key> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut dropped = Vec::new();
        queue.pending.retain(|job| {
            let keep = visible.contains(&job.key);
            if !keep {
                dropped.push(job.key.clone());
            }
            keep
        });
        let added = !fresh.is_empty();
        queue.pending.extend(fresh);
        drop(queue);
        if added {
            self.shared.wake.notify_all();
        }
        dropped
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.closed = true;
        drop(queue);
        self.shared.wake.notify_all();
    }
}

fn worker(
    shared: &Shared,
    decoder: &dyn ThumbDecoder,
    done: &(dyn Fn(Key, Option<Pixels>) + Send + Sync),
) {
    loop {
        let job = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if queue.closed {
                    return;
                }
                if let Some(job) = queue.pending.pop_front() {
                    break job;
                }
                queue = shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        // A decoder that panics must not take the worker (and every later preview) with it.
        let pixels = catch_unwind(AssertUnwindSafe(|| {
            decoder.decode(&job.key.path, job.kind, job.key.px)
        }))
        .ok()
        .flatten();
        done(job.key, pixels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn key(n: i64) -> Key {
        Key {
            path: PathBuf::from(format!("/a/{n}.png")),
            time: n,
            size: 1,
            px: 288,
        }
    }

    fn job(n: i64) -> Job {
        Job {
            key: key(n),
            kind: ThumbKind::Image,
        }
    }

    #[test]
    fn pictures_music_and_video_get_previews() {
        assert_eq!(kind_of("jpg"), Some(ThumbKind::Image));
        assert_eq!(kind_of("HEIC"), Some(ThumbKind::Image));
        assert_eq!(kind_of("mp3"), Some(ThumbKind::Audio));
        assert_eq!(kind_of("mkv"), Some(ThumbKind::Video));
        assert_eq!(kind_of("ts"), Some(ThumbKind::Video));
        assert_eq!(kind_of("txt"), None);
        assert_eq!(kind_of("svg"), None);
        assert_eq!(kind_of(""), None);
    }

    #[test]
    fn only_pictures_have_a_size_limit() {
        assert_eq!(eligible("png", IMAGE_MAX), Some(ThumbKind::Image));
        assert_eq!(eligible("png", IMAGE_MAX + 1), None);
        assert_eq!(eligible("mp4", IMAGE_MAX * 100), Some(ThumbKind::Video));
        assert_eq!(eligible("flac", IMAGE_MAX * 100), Some(ThumbKind::Audio));
    }

    #[test]
    fn decode_size_follows_density() {
        assert_eq!(px_for(1.0), 96);
        assert_eq!(px_for(3.0), 288);
        assert_eq!(px_for(0.0), 1);
    }

    #[test]
    fn sampling_halves_while_both_sides_stay_large() {
        assert_eq!(sample_size(200, 100, 288), 1);
        assert_eq!(sample_size(4000, 3000, 288), 8);
        assert_eq!(sample_size(100, 100, 0), 64);
    }

    #[test]
    fn a_very_wide_picture_is_decoded_smaller_still() {
        assert_eq!(sample_size(20000, 500, 288), 4);
    }

    #[test]
    fn the_oldest_goes_first() {
        let mut c = Cache::new(300, 10);
        c.put(key(1), 1, 100);
        c.put(key(2), 2, 100);
        c.put(key(3), 3, 100);
        assert_eq!(c.put(key(4), 4, 100), Some(vec![key(1)]));
        assert!(!c.contains(&key(1)));
        assert_eq!(c.bytes(), 300);
    }

    #[test]
    fn a_hit_counts_as_use() {
        let mut c = Cache::new(300, 10);
        c.put(key(1), 1, 100);
        c.put(key(2), 2, 100);
        c.put(key(3), 3, 100);
        assert_eq!(c.get(&key(1)), Some(1));
        assert!(c.touch(&key(1)));
        assert_eq!(c.put(key(4), 4, 100), Some(vec![key(2)]));
        assert!(!c.touch(&key(2)));
    }

    #[test]
    fn the_number_of_entries_is_bounded_too() {
        let mut c = Cache::new(1000, 2);
        c.put(key(1), 1, 1);
        c.put(key(2), 2, 1);
        assert_eq!(c.put(key(3), 3, 1), Some(vec![key(1)]));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn a_key_put_again_replaces_its_cost() {
        let mut c = Cache::new(1000, 10);
        c.put(key(1), 1, 100);
        c.put(key(1), 2, 40);
        assert_eq!((c.len(), c.bytes()), (1, 40));
        assert_eq!(c.get(&key(1)), Some(2));
    }

    #[test]
    fn too_big_is_refused_and_clear_empties() {
        let mut c = Cache::new(100, 10);
        assert_eq!(c.put(key(1), 1, 101), None);
        assert!(c.is_empty());
        c.put(key(1), 1, 50);
        c.clear();
        assert_eq!((c.len(), c.bytes()), (0, 0));
    }

    #[test]
    fn retarget_drops_what_is_no_longer_in_view() {
        let pool = Pool::idle();
        assert!(
            pool.retarget(&HashSet::new(), vec![job(1), job(2)])
                .is_empty()
        );
        let visible: HashSet<Key> = [key(2)].into_iter().collect();
        assert_eq!(pool.retarget(&visible, vec![job(3)]), vec![key(1)]);
        let queue = pool.shared.queue.lock().unwrap();
        let left: Vec<i64> = queue.pending.iter().map(|j| j.key.time).collect();
        assert_eq!(left, vec![2, 3]);
    }

    struct Fake;

    impl ThumbDecoder for Fake {
        fn decode(&self, path: &Path, _kind: ThumbKind, _px: u32) -> Option<Pixels> {
            if path.to_string_lossy().contains("bad") {
                return None;
            }
            Some(Pixels {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            })
        }
    }

    #[test]
    fn workers_decode_and_report() {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let done: Done = Arc::new(move |key, pixels| {
            let _ = tx.lock().unwrap().send((key, pixels.is_some()));
        });
        let pool = Pool::start(Arc::new(Fake), done);
        let bad = Job {
            key: Key {
                path: PathBuf::from("/a/bad.png"),
                time: 2,
                size: 1,
                px: 288,
            },
            kind: ThumbKind::Image,
        };
        assert!(pool.retarget(&HashSet::new(), vec![job(1), bad]).is_empty());
        let wait = Duration::from_secs(5);
        let mut got = vec![
            rx.recv_timeout(wait).unwrap(),
            rx.recv_timeout(wait).unwrap(),
        ];
        got.sort_by_key(|(key, _)| key.time);
        let ok: Vec<bool> = got.iter().map(|(_, ok)| *ok).collect();
        assert_eq!(ok, vec![true, false]);
    }
}
