//! A folder as the list shows it: entries, a sorted and filtered view, and a selection.
//!
//! Entries are never moved once loaded, so an entry's *id* (its index in the loaded vector) is
//! stable. The view is a vector of ids in display order; the selection is a set of ids, so it
//! survives re-sorting, filtering and metadata arriving.

use std::collections::HashSet;

use crate::entry::{Entry, Kind, Meta};
use crate::order::{Order, compare};

/// What a change did to the rows of the view, for the UI to forward to its model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Nothing visible changed.
    None,
    /// These rows of the view changed in place.
    Rows(Vec<usize>),
    /// The rows changed their order or number: draw everything again.
    Reset,
}

pub struct Listing {
    entries: Vec<Entry>,
    view: Vec<u32>,
    order: Order,
    show_hidden: bool,
    /// False: only folders and archives are listed.
    show_all: bool,
    query: String,
    selected: HashSet<u32>,
}

impl Listing {
    pub fn new(entries: Vec<Entry>, order: Order) -> Self {
        let mut l = Self {
            entries,
            view: Vec::new(),
            order,
            show_hidden: false,
            show_all: true,
            query: String::new(),
            selected: HashSet::new(),
        };
        l.rebuild();
        l
    }

    // ---- the view -----------------------------------------------------------------------------

    /// Rows in the view.
    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    /// Entries loaded, before filtering.
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    pub fn order(&self) -> Order {
        self.order
    }

    /// The id of the entry shown in `row`.
    pub fn id_at(&self, row: usize) -> Option<u32> {
        self.view.get(row).copied()
    }

    pub fn entry(&self, row: usize) -> Option<&Entry> {
        let id = self.id_at(row)?;
        self.entries.get(id as usize)
    }

    pub fn entry_by_id(&self, id: u32) -> Option<&Entry> {
        self.entries.get(id as usize)
    }

    pub fn set_order(&mut self, order: Order) {
        self.order = order;
        self.rebuild();
    }

    pub fn set_show_hidden(&mut self, on: bool) {
        self.show_hidden = on;
        self.rebuild();
    }

    pub fn set_show_all(&mut self, on: bool) {
        self.show_all = on;
        self.rebuild();
    }

    /// Words that must all occur in a name, case-insensitively. Empty shows everything.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_lowercase();
        self.rebuild();
    }

    /// A search is filtering the view.
    pub fn has_query(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn shows_all(&self) -> bool {
        self.show_all
    }

    fn rebuild(&mut self) {
        let words: Vec<&str> = self.query.split_whitespace().collect();
        let (hidden, all) = (self.show_hidden, self.show_all);
        let mut ids: Vec<u32> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if !hidden && e.is_hidden() {
                    return false;
                }
                if !all && !matches!(e.kind, Kind::Dir | Kind::Set | Kind::Archive) {
                    return false;
                }
                if words.is_empty() {
                    return true;
                }
                let lower = e.name.to_lowercase();
                words.iter().all(|w| lower.contains(w))
            })
            .map(|(i, _)| i as u32)
            .collect();
        let (entries, order) = (&self.entries, self.order);
        ids.sort_by(|&a, &b| compare(&entries[a as usize], &entries[b as usize], order));
        self.view = ids;
    }

    // ---- metadata arriving ----------------------------------------------------------------------

    /// Ids of up to `count` rows from `first` that still have no metadata, in display order: what
    /// to fetch next, so that what is on screen fills in first.
    pub fn missing_meta(&self, first: usize, count: usize) -> Vec<u32> {
        self.view
            .iter()
            .skip(first)
            .take(count)
            .copied()
            .filter(|&id| self.entries[id as usize].meta.is_none())
            .collect()
    }

    /// Ids of folders in `first..first + count` whose item count is not known yet.
    pub fn missing_counts(&self, first: usize, count: usize) -> Vec<u32> {
        self.view
            .iter()
            .skip(first)
            .take(count)
            .copied()
            .filter(|&id| {
                let e = &self.entries[id as usize];
                e.kind.is_folder() && e.children.is_none()
            })
            .collect()
    }

    pub fn apply_meta(&mut self, updates: &[(u32, Meta)]) -> Change {
        let mut ids = HashSet::new();
        for &(id, meta) in updates {
            if let Some(e) = self.entries.get_mut(id as usize) {
                e.meta = Some(meta);
                ids.insert(id);
            }
        }
        if ids.is_empty() {
            return Change::None;
        }
        if self.order.key.needs_meta() {
            self.rebuild();
            return Change::Reset;
        }
        self.rows_of(&ids)
    }

    pub fn apply_counts(&mut self, updates: &[(u32, u32)]) -> Change {
        let mut ids = HashSet::new();
        for &(id, n) in updates {
            if let Some(e) = self.entries.get_mut(id as usize) {
                e.children = Some(n);
                ids.insert(id);
            }
        }
        if ids.is_empty() {
            return Change::None;
        }
        self.rows_of(&ids)
    }

    fn rows_of(&self, ids: &HashSet<u32>) -> Change {
        let rows: Vec<usize> = self
            .view
            .iter()
            .enumerate()
            .filter(|(_, id)| ids.contains(id))
            .map(|(row, _)| row)
            .collect();
        if rows.is_empty() {
            Change::None
        } else {
            Change::Rows(rows)
        }
    }

    // ---- selection --------------------------------------------------------------------------

    pub fn is_selected(&self, row: usize) -> bool {
        self.id_at(row)
            .is_some_and(|id| self.selected.contains(&id))
    }

    /// Flip one row; returns whether it is selected now.
    pub fn toggle(&mut self, row: usize) -> bool {
        let Some(id) = self.id_at(row) else {
            return false;
        };
        if !self.selected.remove(&id) {
            self.selected.insert(id);
            return true;
        }
        false
    }

    pub fn select_all(&mut self) {
        self.selected.extend(self.view.iter().copied());
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn is_selecting(&self) -> bool {
        !self.selected.is_empty()
    }

    /// The names of everything selected, including what a filter hides at the moment. A folder
    /// that is read again numbers its entries afresh, so a selection that has to outlive that
    /// travels by name.
    pub fn selected_names(&self) -> HashSet<String> {
        self.selected
            .iter()
            .filter_map(|&id| self.entries.get(id as usize))
            .map(|e| e.name.clone())
            .collect()
    }

    /// Select the entries with these names (those that are gone are skipped).
    pub fn select_names(&mut self, names: &HashSet<String>) {
        let ids = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| names.contains(&e.name))
            .map(|(i, _)| i as u32);
        self.selected.extend(ids);
    }

    /// The selected entries in display order (those hidden by a filter are not included).
    pub fn selected_entries(&self) -> Vec<&Entry> {
        self.view
            .iter()
            .filter(|id| self.selected.contains(id))
            .map(|&id| &self.entries[id as usize])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::SortKey;

    fn sample() -> Listing {
        Listing::new(
            vec![
                Entry::with_meta("b.txt", Kind::File, 10, 100),
                Entry::new("Music", Kind::Dir),
                Entry::with_meta(".hidden", Kind::File, 1, 1),
                Entry::with_meta("a10.png", Kind::File, 30, 300),
                Entry::with_meta("a2.png", Kind::File, 20, 200),
                Entry::with_meta("pack.kiln", Kind::Archive, 99, 50),
            ],
            Order::default(),
        )
    }

    fn row_names(l: &Listing) -> Vec<String> {
        (0..l.len())
            .map(|r| l.entry(r).unwrap().name.clone())
            .collect()
    }

    #[test]
    fn hidden_are_left_out_and_folders_lead() {
        let l = sample();
        assert_eq!(l.total(), 6);
        assert_eq!(
            row_names(&l),
            ["Music", "a2.png", "a10.png", "b.txt", "pack.kiln"]
        );
    }

    #[test]
    fn show_hidden_and_archives_only() {
        let mut l = sample();
        l.set_show_hidden(true);
        assert_eq!(l.len(), 6);
        l.set_show_all(false);
        assert_eq!(row_names(&l), ["Music", "pack.kiln"]);
    }

    #[test]
    fn every_query_word_must_match() {
        let mut l = sample();
        l.set_query("PNG a1");
        assert_eq!(row_names(&l), ["a10.png"]);
        l.set_query("");
        assert_eq!(l.len(), 5);
    }

    #[test]
    fn selection_survives_a_resort() {
        let mut l = sample();
        // row 1 is a2.png
        assert!(l.toggle(1));
        let id = l.id_at(1).unwrap();
        l.set_order(Order {
            key: SortKey::Size,
            ascending: false,
        });
        let row = (0..l.len()).find(|&r| l.id_at(r) == Some(id)).unwrap();
        assert!(l.is_selected(row));
        assert_eq!(l.selected_count(), 1);
        assert!(!l.toggle(row));
        assert!(!l.is_selecting());
    }

    #[test]
    fn a_selection_travels_by_name_to_a_listing_read_again() {
        let mut l = sample();
        l.toggle(1); // a2.png
        l.toggle(0); // Music
        let names = l.selected_names();
        assert_eq!(names.len(), 2);

        // the same folder read again, a file gone and the order of the entries different
        let mut again = Listing::new(
            vec![
                Entry::new("Music", Kind::Dir),
                Entry::with_meta("a2.png", Kind::File, 20, 200),
                Entry::with_meta("new.txt", Kind::File, 1, 1),
            ],
            Order::default(),
        );
        again.select_names(&names);
        assert_eq!(again.selected_count(), 2);
        assert!(again.is_selected(0));
        assert!(again.is_selected(1));
        assert!(!again.is_selected(2));

        // a name that is not there any more selects nothing
        let mut gone = HashSet::new();
        gone.insert("deleted.bin".to_string());
        again.clear_selection();
        again.select_names(&gone);
        assert!(!again.is_selecting());
    }

    #[test]
    fn a_hidden_selection_is_still_named() {
        let mut l = sample();
        l.select_all();
        l.set_query("png");
        assert_eq!(l.len(), 2);
        // five were selected, and the filter only hides three of them
        assert_eq!(l.selected_names().len(), 5);
        assert!(l.has_query());
        l.set_query("  ");
        assert!(!l.has_query());
    }

    #[test]
    fn what_a_search_hides_is_never_in_the_entries_to_delete() {
        let mut l = sample();
        l.select_all();
        l.set_query("png");
        // the bar still counts all five, but only the two rows in view can be acted on
        assert_eq!(l.selected_names().len(), 5);
        let mut names: Vec<&str> = l
            .selected_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["a10.png", "a2.png"]);
    }

    #[test]
    fn select_all_takes_only_the_view() {
        let mut l = sample();
        l.select_all();
        assert_eq!(l.selected_count(), 5);
        l.clear_selection();
        assert_eq!(l.selected_count(), 0);
    }

    #[test]
    fn metadata_arrives_for_the_rows_in_view_first() {
        let mut l = Listing::new(
            vec![
                Entry::new("a", Kind::File),
                Entry::new("b", Kind::File),
                Entry::new("c", Kind::File),
            ],
            Order::default(),
        );
        assert_eq!(l.missing_meta(1, 5), [1, 2]);
        let change = l.apply_meta(&[(1, Meta { size: 5, mtime: 7 })]);
        assert_eq!(change, Change::Rows(vec![1]));
        assert_eq!(l.missing_meta(0, 5), [0, 2]);
        assert_eq!(l.apply_meta(&[(99, Meta::default())]), Change::None);
    }

    #[test]
    fn metadata_resets_a_size_sorted_view() {
        let mut l = Listing::new(
            vec![Entry::new("a", Kind::File), Entry::new("b", Kind::File)],
            Order {
                key: SortKey::Size,
                ascending: false,
            },
        );
        let change = l.apply_meta(&[(1, Meta { size: 9, mtime: 0 })]);
        assert_eq!(change, Change::Reset);
        assert_eq!(row_names(&l), ["b", "a"]);
    }

    #[test]
    fn counts_fill_in_for_folders_only() {
        let mut l = Listing::new(
            vec![Entry::new("d", Kind::Dir), Entry::new("f", Kind::File)],
            Order::default(),
        );
        assert_eq!(l.missing_counts(0, 10), [0]);
        assert_eq!(l.apply_counts(&[(0, 23)]), Change::Rows(vec![0]));
        assert!(l.missing_counts(0, 10).is_empty());
        assert_eq!(l.entry(0).unwrap().children, Some(23));
    }
}
