// SPDX-License-Identifier: MIT
//! The names file: titles and labels for the catalog, read from one JSON file.
//!
//! The library ships no game text. A names file supplies it; `--names FILE` picks one and
//! [`default_path`] finds one when the switch is absent. The schema is in `docs/player.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::catalog::Naming;

/// The schema version this reader understands.
pub const VERSION: u32 = 1;

/// The file name [`default_path`] looks for.
pub const DEFAULT_FILE: &str = "bof3-names.json";

/// Where a name comes from, from most to least trusted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A table or header in the game files.
    Game,
    /// In-game text that names the item.
    Text,
    /// A published data set that carries a reference.
    Data,
    /// A neutral label built from where the item plays; not a title.
    Label,
    /// No source names the item.
    #[default]
    None,
}

/// One named item.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Entry {
    /// The title or label. Absent for a copy that shares its original's title.
    pub title: Option<String>,
    /// Where the title comes from.
    #[serde(default)]
    pub source: Source,
    /// A short pointer to the evidence: a file and offset, or a table and symbol.
    pub evidence: Option<String>,
    /// Where the item plays, when the title is a label.
    pub area: Option<String>,
    /// The id of the item whose content this one repeats.
    pub same_as: Option<String>,
    /// The game's sound ids that select this music.
    #[serde(default)]
    pub sound: Vec<u16>,
    /// The ability indexes a magic bank serves, in the order of the names in `title`.
    #[serde(default)]
    pub abilities: Vec<u16>,
}

/// The names of one file, keyed by catalog id.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Names {
    version: u32,
    #[serde(default)]
    music: BTreeMap<String, Entry>,
    #[serde(default)]
    banks: BTreeMap<String, Entry>,
    #[serde(default)]
    xa: BTreeMap<String, Entry>,
}

impl Names {
    /// Parse a names file.
    ///
    /// # Errors
    /// Returns the reason when the text is not a names file or its version is not [`VERSION`].
    pub fn from_json(text: &str) -> Result<Self, String> {
        let names: Self = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if names.version == VERSION {
            Ok(names)
        } else {
            Err(format!("unsupported names version {} (this player reads version {VERSION})", names.version))
        }
    }

    /// Read the names file at `path`.
    ///
    /// # Errors
    /// Returns the reason when the file cannot be read or parsed.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::from_json(&text)
    }

    /// The music entries, keyed `music:ARCHIVE#SEQUENCE`.
    #[must_use]
    pub fn music(&self) -> &BTreeMap<String, Entry> {
        &self.music
    }

    /// The bank entries, keyed `bank:ARCHIVE`.
    #[must_use]
    pub fn banks(&self) -> &BTreeMap<String, Entry> {
        &self.banks
    }

    /// The XA entries, keyed by catalog id.
    #[must_use]
    pub fn xa(&self) -> &BTreeMap<String, Entry> {
        &self.xa
    }

    fn music_entry(&self, slot: &str) -> Option<&Entry> {
        self.music.get(&format!("music:{slot}"))
    }

    fn bank_entry(&self, archive: &str) -> Option<&Entry> {
        self.banks.get(&format!("bank:{archive}"))
    }
}

/// The names file to use when `--names` is absent: `$BOF3_NAMES`, then `bof3-names.json`,
/// `build/bof3-names.json` or `data/bof3-names.json` in the working directory and its parents,
/// then in the executable's directory and its parents. `None` when none exists.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("BOF3_NAMES").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok().and_then(|path| path.parent().map(Path::to_path_buf));
    let bases =
        cwd.iter().flat_map(|dir| dir.ancestors().take(6)).chain(exe.iter().flat_map(|dir| dir.ancestors().take(4)));
    bases
        .flat_map(|base| {
            [base.join(DEFAULT_FILE), base.join("build").join(DEFAULT_FILE), base.join("data").join(DEFAULT_FILE)]
        })
        .find(|path| path.is_file())
}

/// The names file `--names` selects, or else the default one. A file that was asked for must
/// read; a default file that does not read gives the reason as the second value and no names.
#[must_use]
pub fn load(explicit: Option<&Path>) -> (Names, Option<String>) {
    let (path, asked) = match explicit {
        Some(path) => (Some(path.to_path_buf()), true),
        None => (default_path(), false),
    };
    let Some(path) = path else { return (Names::default(), None) };
    match Names::read(&path) {
        Ok(names) => (names, None),
        Err(error) => {
            let kind = if asked { "names file" } else { "default names file" };
            (Names::default(), Some(format!("cannot read the {kind} {}: {error}", path.display())))
        }
    }
}

impl Naming for Names {
    fn xa_span(&self, id: &str) -> Option<&str> {
        self.xa.get(id)?.title.as_deref()
    }

    fn music_copy(&self, slot: &str) -> Option<&str> {
        self.music_entry(slot)?.same_as.as_deref()?.strip_prefix("music:")
    }

    fn music_sound(&self, slot: &str) -> Option<u16> {
        self.music_entry(slot)?.sound.first().copied()
    }

    fn bank_name(&self, path: &str) -> Option<&str> {
        let file = path.rsplit('/').next()?;
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        self.bank_entry(stem)?.title.as_deref()
    }

    fn bank_duplicate(&self, archive: &str) -> Option<&str> {
        self.bank_entry(archive)?.same_as.as_deref()?.strip_prefix("bank:")
    }

    fn magic_ability(&self, ability: u16) -> Option<&str> {
        self.banks.values().find_map(|entry| {
            let index = entry.abilities.iter().position(|&value| value == ability)?;
            entry.title.as_deref()?.split(", ").nth(index)
        })
    }

    fn music_title(&self, archive: &str, sequence: u32) -> Option<&str> {
        let mut entry = self.music_entry(&format!("{archive}#{sequence}"))?;
        // The original of a copy is never itself a copy; the bound only guards a bad file.
        for _ in 0..4 {
            if let Some(title) = entry.title.as_deref() {
                return Some(title);
            }
            entry = self.music.get(entry.same_as.as_deref()?)?;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
 "version":1,
 "music":{
  "music:A#0":{"title":"First","source":"data","sound":[3,9]},
  "music:B#0":{"same_as":"music:A#0"},
  "music:C#1":{"title":"Own","source":"label","same_as":"music:A#0"},
  "music:D#0":{"source":"none"}
 },
 "banks":{
  "bank:MAGIC001":{"title":"Jump, Nue Stomp","source":"game","abilities":[1,160]},
  "bank:AREA001":{"same_as":"bank:AREA000"}
 },
 "xa":{"xa:S#ch0@0":{"title":"Cue 0","source":"game"}}
}"#;

    #[test]
    fn copy_takes_the_title_of_its_original() {
        let names = Names::from_json(SAMPLE).unwrap();
        assert_eq!(names.music_title("A", 0), Some("First"));
        assert_eq!(names.music_title("B", 0), Some("First"));
        assert_eq!(names.music_copy("B#0"), Some("A#0"));
        assert_eq!(names.music_title("C", 1), Some("Own"));
        assert_eq!(names.music_title("D", 0), None);
        assert_eq!(names.music_title("Z", 0), None);
    }

    #[test]
    fn lookups_read_every_section() {
        let names = Names::from_json(SAMPLE).unwrap();
        assert_eq!(names.music_sound("A#0"), Some(3));
        assert_eq!(names.bank_name("BIN/BMAGIC/MAGIC001.EMI"), Some("Jump, Nue Stomp"));
        assert_eq!(names.bank_duplicate("AREA001"), Some("AREA000"));
        assert_eq!(names.magic_ability(160), Some("Nue Stomp"));
        assert_eq!(names.xa_span("xa:S#ch0@0"), Some("Cue 0"));
        assert_eq!(names.music()["music:D#0"].source, Source::None);
    }

    #[test]
    fn rejects_other_versions_and_bad_text() {
        assert!(Names::from_json(r#"{"version":2}"#).is_err());
        assert!(Names::from_json("A#0\tTitle").is_err());
        assert!(Names::from_json(r#"{"version":1,"music":{"music:A#0":{"source":"web"}}}"#).is_err());
    }

    /// The file the repository ships, `data/bof3-names.json`.
    const SHIPPED: &str = include_str!("../../data/bof3-names.json");

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bof3player-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_shipped_file_reads_and_every_title_has_a_source() {
        let names = Names::from_json(SHIPPED).unwrap();
        assert!(!names.music().is_empty());
        for (id, entry) in names.music().iter().chain(names.banks()).chain(names.xa()) {
            if entry.title.is_some() {
                assert_ne!(entry.source, Source::None, "{id} has a title and no source");
                assert!(entry.evidence.is_some(), "{id} has a title and no evidence");
            }
        }
    }

    #[test]
    fn every_same_as_in_the_shipped_file_stays_in_its_section() {
        let names = Names::from_json(SHIPPED).unwrap();
        // A music original is always listed; a bank original may be an archive without an entry.
        for (id, entry) in names.music() {
            if let Some(original) = &entry.same_as {
                assert!(names.music().contains_key(original), "{id} points at the missing {original}");
                assert_ne!(original, id, "{id} copies itself");
            }
        }
        for (id, entry) in names.banks() {
            if let Some(original) = &entry.same_as {
                assert!(original.starts_with("bank:") && original != id, "{id} points at {original}");
            }
        }
    }

    #[test]
    fn a_chain_of_copies_ends_and_a_loop_gives_no_title() {
        let names = Names::from_json(
            r#"{"version":1,"music":{
              "music:A#0":{"same_as":"music:B#0"},
              "music:B#0":{"same_as":"music:A#0"},
              "music:C#0":{"same_as":"music:Z#0"}}}"#,
        )
        .unwrap();
        assert_eq!(names.music_title("A", 0), None);
        assert_eq!(names.music_title("C", 0), None);
    }

    #[test]
    fn an_explicit_file_overrides_the_default_and_a_bad_one_reports_why() {
        let dir = scratch("names-override");
        let file = dir.join("mine.json");
        std::fs::write(&file, r#"{"version":1,"music":{"music:A#0":{"title":"Mine","source":"data","evidence":"e"}}}"#)
            .unwrap();
        let (names, failure) = load(Some(&file));
        assert!(failure.is_none());
        assert_eq!(names.music_title("A", 0), Some("Mine"));

        let missing = dir.join("absent.json");
        let (names, failure) = load(Some(&missing));
        let message = failure.expect("a file that was asked for must read");
        assert!(message.contains("names file") && message.contains("absent.json"), "{message}");
        assert!(names.music().is_empty());

        std::fs::write(&file, r#"{"version":9}"#).unwrap();
        let message = load(Some(&file)).1.expect("an unsupported version is reported");
        assert!(message.contains("unsupported names version 9"), "{message}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
