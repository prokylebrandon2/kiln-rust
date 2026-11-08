//! Thumbnail previews on screen: which rows want one, asking the workers for the
//! ones in view, and putting the results into the list. The rules, the cache and the workers are
//! in [`crate::thumbs`]; the decoding is the phone's ([`crate::Host::thumb_decoder`]).
//!
//! Everything here runs on the UI thread, except turning a decoded result into Slint's pixel
//! buffer (a copy of up to a megabyte), which the worker does before handing the result over. A
//! result arrives through `invoke_from_event_loop` and is looked up by file, modification time,
//! size and decode size; it is dropped when no row wants it any more. The list never holds a
//! picture the cache does not: what the cache lets go of is taken out of the rows too, so the
//! cache's limit is the limit of the memory the previews use.

use std::collections::HashSet;
use std::sync::Arc;

use kiln_ui::PreviewData;
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer};

use crate::browser::{Browser, Kind, Row};
use crate::thumbs::{self, Cache, Done, Job, Key, Pixels, Pool, ThumbKind};
use crate::{App, app_do};

/// What the cache may hold, in bytes of pixels.
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const CACHE_ENTRIES: usize = 2048;
/// What an entry costs besides its pixels, so that a long run of failures is bounded too.
const ENTRY_COST: usize = 256;
/// The slots asked for before the list has said how many it shows.
const FIRST_SLOTS: i32 = 24;

/// What the list keeps about previews.
pub(crate) struct ThumbUi {
    /// Absent when the phone cannot decode: then no row has a preview.
    pool: Option<Pool>,
    /// Is a file that has no preview.
    cache: Cache<Option<Image>>,
    /// Asked for and not back yet: a file is never asked for twice at the same time.
    in_flight: HashSet<Key>,
    /// The decode size now (96 dp).
    px: u32,
    /// The slots the list has built (see `KFileList.visible-changed`).
    first: i32,
    shown: i32,
}

impl Default for ThumbUi {
    fn default() -> Self {
        Self {
            pool: None,
            cache: Cache::new(CACHE_BYTES, CACHE_ENTRIES),
            in_flight: HashSet::new(),
            px: thumbs::px_for(1.0),
            first: 0,
            shown: FIRST_SLOTS,
        }
    }
}

/// What a row would be asked for, or `None` when it only gets its icon.
fn thumb_key(b: &Browser, row: &Row, px: u32) -> Option<(Key, ThumbKind)> {
    if row.kind != Kind::File {
        return None;
    }
    let kind = thumbs::eligible(&row.ext, row.size)?;
    let key = Key {
        path: b.cwd.join(&row.key),
        time: row.time,
        size: row.size,
        px,
    };
    Some((key, kind))
}

/// A decoded preview as Slint's pixel buffer. Unlike an [`Image`] the buffer may cross threads, so
/// a worker builds it and the UI thread only wraps it (`Image::from_rgba8` does not copy).
struct Ready {
    buffer: SharedPixelBuffer<Rgba8Pixel>,
    /// What the pixels weigh, for the cache.
    bytes: usize,
}

/// `None` when the pixels do not fit their size (a decoder that got it wrong): the file then gets
/// no preview, as for any other failure.
fn ready_of(p: Pixels) -> Option<Ready> {
    let want = (p.width as usize)
        .checked_mul(p.height as usize)?
        .checked_mul(4)?;
    if p.width == 0 || p.height == 0 || p.rgba.len() != want {
        return None;
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(p.width, p.height);
    buffer.make_mut_bytes().copy_from_slice(&p.rgba);
    Some(Ready {
        buffer,
        bytes: want,
    })
}

impl App {
    pub(crate) fn wire_thumbs(&self) {
        self.ui
            .on_list_visible(|first, shown| app_do(|app| app.thumb_visible(first, shown)));
        self.thumb_look();
        let Some(decoder) = self.host.thumb_decoder() else {
            return;
        };
        let done: Done = Arc::new(|key: Key, pixels: Option<Pixels>| {
            // This runs on the worker: the copy into Slint's buffer is made here, not on the UI
            // thread.
            let ready = pixels.and_then(ready_of);
            // The window may be gone by now; then there is nobody to tell.
            let _ = slint::invoke_from_event_loop(move || {
                app_do(|app| app.thumb_done(key, ready));
            });
        });
        self.thumbs.borrow_mut().pool = Some(Pool::start(decoder, done));
    }

    /// The density changed (or is known for the first time): previews are decoded for it.
    pub(crate) fn thumb_look(&self) {
        let px = thumbs::px_for(self.host.look().density);
        self.thumbs.borrow_mut().px = px;
    }

    /// The size to decode at, or `None` when previews are off (`previews`, on by default), an
    /// archive is open, or the phone cannot decode.
    pub(crate) fn thumb_px(&self, b: &Browser) -> Option<u32> {
        if b.arc.is_some() || !self.prefs.bool("previews", true) {
            return None;
        }
        let t = self.thumbs.borrow();
        t.pool.as_ref().map(|_| t.px)
    }

    /// The preview of a row if it is in the cache.
    pub(crate) fn thumb_cached(&self, b: &Browser, row: &Row, px: Option<u32>) -> Option<Image> {
        let (key, _) = thumb_key(b, row, px?)?;
        self.thumbs.borrow_mut().cache.get(&key).flatten()
    }

    /// The rows in view that want a preview.
    fn thumb_wanted(&self) -> Vec<(Key, ThumbKind)> {
        let b = self.browser.borrow();
        let Some(px) = self.thumb_px(&b) else {
            return Vec::new();
        };
        let (first, shown) = {
            let t = self.thumbs.borrow();
            (t.first, t.shown)
        };
        // The ".." row is a slot of its own.
        let up = i32::from(b.up_shown());
        let lo = usize::try_from((first - up).max(0)).unwrap_or(0);
        let hi = usize::try_from((first + shown - up).max(0))
            .unwrap_or(0)
            .min(b.rows.len());
        b.rows
            .iter()
            .take(hi)
            .skip(lo)
            .filter_map(|row| thumb_key(&b, row, px))
            .collect()
    }

    /// Ask for the previews of the rows in view that are neither cached nor on their way, and
    /// forget the requests that have not started for rows that are no longer in view.
    pub(crate) fn thumb_request(&self) {
        let wanted = self.thumb_wanted();
        let mut guard = self.thumbs.borrow_mut();
        let t = &mut *guard;
        let Some(pool) = t.pool.as_ref() else {
            return;
        };
        let visible: HashSet<Key> = wanted.iter().map(|(key, _)| key.clone()).collect();
        let mut fresh = Vec::new();
        for (key, kind) in wanted {
            // a hit also keeps the entry from being the next one to go
            if t.cache.touch(&key) || t.in_flight.contains(&key) {
                continue;
            }
            fresh.push(Job { key, kind });
        }
        for key in pool.retarget(&visible, fresh.clone()) {
            t.in_flight.remove(&key);
        }
        for job in fresh {
            t.in_flight.insert(job.key);
        }
    }

    fn thumb_visible(&self, first: i32, shown: i32) {
        {
            let mut t = self.thumbs.borrow_mut();
            t.first = first;
            t.shown = shown;
        }
        self.thumb_request();
    }

    /// A worker finished `key`. Failures are cached too, so a file with no preview is not tried
    /// again.
    fn thumb_done(&self, key: Key, ready: Option<Ready>) {
        let cost = ENTRY_COST + ready.as_ref().map_or(0, |r| r.bytes);
        let image = ready.map(|r| Image::from_rgba8(r.buffer));
        let evicted = {
            let mut t = self.thumbs.borrow_mut();
            t.in_flight.remove(&key);
            t.cache.put(key.clone(), image.clone(), cost)
        };
        let Some(evicted) = evicted else {
            return;
        };
        self.thumb_detach(&evicted);
        if evicted.contains(&key) {
            return;
        }
        if let Some(image) = image {
            self.thumb_show(&key, &image);
        }
    }

    /// The index of the row (and of its list entries) a key belongs to, if it is still on screen
    /// at that size.
    fn thumb_row_of(&self, b: &Browser, key: &Key) -> Option<usize> {
        if self.thumb_px(b) != Some(key.px) || key.path.parent() != Some(b.cwd.as_path()) {
            return None;
        }
        let name = key.path.file_name()?.to_str()?;
        let i = b
            .rows
            .iter()
            .position(|r| r.key == name && r.time == key.time && r.size == key.size)?;
        let row = self.model.row_data(i)?;
        (row.name.as_str() == b.rows[i].title && i < self.previews.row_count()).then_some(i)
    }

    fn thumb_show(&self, key: &Key, image: &Image) {
        let b = self.browser.borrow();
        if let Some(i) = self.thumb_row_of(&b, key) {
            self.previews.set_row_data(
                i,
                PreviewData {
                    image: image.clone(),
                    has: true,
                },
            );
        }
    }

    /// Take the previews the cache let go of out of the rows (the icon shows again).
    fn thumb_detach(&self, keys: &[Key]) {
        if keys.is_empty() {
            return;
        }
        let b = self.browser.borrow();
        for key in keys {
            if let Some(i) = self.thumb_row_of(&b, key)
                && self.previews.row_data(i).is_some_and(|p| p.has)
            {
                self.previews.set_row_data(i, PreviewData::default());
            }
        }
    }

    /// The system is short of memory: let go of every preview (the ones in view are decoded again).
    pub(crate) fn thumb_trim(&self) {
        self.thumbs.borrow_mut().cache.clear();
        self.render_rows();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(width: u32, height: u32, len: usize) -> Pixels {
        Pixels {
            width,
            height,
            rgba: vec![7; len],
        }
    }

    #[test]
    fn a_decoded_preview_keeps_its_size_and_pixels() {
        let ready = ready_of(pixels(3, 2, 24)).expect("fits");
        assert_eq!(ready.bytes, 24);
        assert_eq!((ready.buffer.width(), ready.buffer.height()), (3, 2));
        assert!(ready.buffer.as_bytes().iter().all(|b| *b == 7));
    }

    #[test]
    fn pixels_that_do_not_fit_their_size_give_no_preview() {
        assert!(ready_of(pixels(3, 2, 23)).is_none());
        assert!(ready_of(pixels(3, 2, 25)).is_none());
        assert!(ready_of(pixels(0, 2, 0)).is_none());
        assert!(ready_of(pixels(2, 0, 0)).is_none());
        // a size whose byte count overflows is refused, not wrapped
        assert!(ready_of(pixels(u32::MAX, u32::MAX, 4)).is_none());
    }

    /// The buffer is what a worker hands to the UI thread, so it has to be `Send`.
    #[test]
    fn a_ready_preview_can_cross_threads() {
        fn is_send<T: Send>() {}
        is_send::<Ready>();
    }
}
