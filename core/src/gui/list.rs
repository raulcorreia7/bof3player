// SPDX-License-Identifier: MIT
//! The catalog as a tree of kind, family and item rows, like the terminal browser's.

use std::collections::{BTreeMap, HashSet};

use crate::catalog::{Filter, Item, Kind};
use crate::library::Library;

/// Most search hits listed.
const MAX_HITS: usize = 5000;
/// The text of an item row: its title or name, else its id.
pub fn label(item: &Item) -> &str {
    [&item.title, &item.name, &item.id].into_iter().find(|text| !text.is_empty()).map_or("", |text| text.as_str())
}

/// One line of the list.
#[derive(Clone, Copy)]
pub enum Row {
    /// A kind and the items under it.
    Kind(Kind, usize),
    /// A family of one kind and the items under it.
    Family(Kind, &'static str, usize),
    /// An item.
    Item(&'static Item),
}

impl Row {
    /// A key that names the row across rebuilds.
    pub fn key(&self) -> String {
        match self {
            Row::Kind(kind, _) => kind_key(*kind),
            Row::Family(kind, family, _) => family_key(*kind, family),
            Row::Item(item) => item.id.clone(),
        }
    }
}

fn kind_key(kind: Kind) -> String {
    kind.name().to_string()
}

fn family_key(kind: Kind, family: &str) -> String {
    format!("{}/{family}", kind.name())
}

/// All items grouped by kind and family, and which groups are open.
pub struct Tree {
    library: &'static Library<'static>,
    groups: BTreeMap<Kind, BTreeMap<&'static str, Vec<&'static Item>>>,
    expanded: HashSet<String>,
    /// How many playable items the catalog holds.
    pub playable: usize,
}

impl Tree {
    /// Group the catalog of `library`. Music and its families start open.
    pub fn new(library: &'static Library<'static>) -> Self {
        let mut groups: BTreeMap<Kind, BTreeMap<&str, Vec<&Item>>> = BTreeMap::new();
        for item in library.catalog().items() {
            groups.entry(item.kind).or_default().entry(item.family.as_str()).or_default().push(item);
        }
        let mut expanded = HashSet::new();
        if let Some(families) = groups.get(&Kind::Music) {
            expanded.insert(kind_key(Kind::Music));
            expanded.extend(families.keys().map(|family| family_key(Kind::Music, family)));
        }
        Self { library, groups, expanded, playable: library.catalog().playable_count() }
    }

    /// The number of catalog items.
    pub fn total(&self) -> usize {
        self.library.catalog().len()
    }

    /// Whether the group named by `row` is open.
    pub fn is_open(&self, row: &Row) -> bool {
        self.expanded.contains(&row.key())
    }

    /// Open or close the group `row` names; `open` forces the state. Items do not toggle.
    pub fn set_open(&mut self, row: &Row, open: Option<bool>) {
        if matches!(row, Row::Item(_)) {
            return;
        }
        let key = row.key();
        let open = open.unwrap_or(!self.expanded.contains(&key));
        if open {
            self.expanded.insert(key);
        } else {
            self.expanded.remove(&key);
        }
    }

    /// The visible rows. A non-empty `query` lists the ranked hits under their kinds instead of
    /// the families.
    pub fn rows(&self, query: &str, playable_only: bool) -> Vec<Row> {
        let query = query.trim();
        let mut rows = Vec::new();
        if query.is_empty() {
            for kind in Kind::LIST {
                let Some(families) = self.groups.get(&kind) else { continue };
                let count = |items: &Vec<&Item>| items.iter().filter(|item| !playable_only || item.playable).count();
                let total: usize = families.values().map(count).sum();
                if total == 0 {
                    continue;
                }
                rows.push(Row::Kind(kind, total));
                if !self.expanded.contains(&kind_key(kind)) {
                    continue;
                }
                for (&family, items) in families {
                    let shown = count(items);
                    if shown == 0 {
                        continue;
                    }
                    rows.push(Row::Family(kind, family, shown));
                    if self.expanded.contains(&family_key(kind, family)) {
                        rows.extend(items.iter().filter(|item| !playable_only || item.playable).map(|&i| Row::Item(i)));
                    }
                }
            }
            return rows;
        }
        let filter = Filter { playable_only, ..Filter::default() };
        let hits = self.library.catalog().search(query, &filter);
        for kind in Kind::LIST {
            let items: Vec<&Item> = hits.iter().copied().filter(|item| item.kind == kind).take(MAX_HITS).collect();
            if !items.is_empty() {
                rows.push(Row::Kind(kind, items.len()));
                rows.extend(items.into_iter().map(Row::Item));
            }
        }
        rows
    }
}
