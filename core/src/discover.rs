// SPDX-License-Identifier: MIT
//! Finding the game data.
//!
//! Probes, in order: the explicit hint and `$BOF3_GAME_DIR` (each as itself
//! and as `<value>/out/extracted`), the working directory and
//! its parents, then the executable's directory and its parents; each of those folders is probed
//! as itself and as `out/extracted`, `rebof3/build/disc` (the monorepo's disc), `BIN` and
//! `inputs/external` below it. A candidate is
//! accepted when it is an extracted tree whose `BIN/BGM` holds an `.EMI`
//! archive (also when the path names `BIN`, an asset directory or one archive:
//! discovery walks up to three levels to the root), or a `.cue`/disc image whose
//! ISO 9660 root has a `BIN` directory.

use std::path::{Path, PathBuf};

use crate::Library;
use crate::catalog::Naming;
use crate::error::{Error, Result};
use crate::source::{DiscSource, FolderSource, Source};

/// How many levels above a hint the tree root may sit.
const ROOT_ASCENT: usize = 3;
/// Parents of the working directory that are probed.
const WORKING_PARENTS: usize = 5;
/// Parents of the executable directory that are probed.
const EXECUTABLE_PARENTS: usize = 3;

/// What kind of data a probe accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An extracted folder.
    Folder,
    /// A disc image.
    Disc,
}

/// One probed location.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Why it was probed (`hint`, `BOF3_GAME_DIR`, `cwd/..`, ...).
    pub label: String,
    /// The path tried.
    pub path: PathBuf,
    /// Accepted as the data source.
    pub accepted: bool,
    /// What was found, or why it was rejected.
    pub detail: String,
}

/// The outcome of [`discover`].
#[derive(Debug, Clone)]
pub struct Discovery {
    /// Every probe made, in order, up to and including the accepted one.
    pub candidates: Vec<Candidate>,
    /// The accepted source, if any.
    pub found: Option<(SourceKind, PathBuf)>,
}

impl Discovery {
    /// Open the discovered source as a library with ids and archive names only.
    pub fn open(&self) -> Result<Library<'static>> {
        self.open_with(&crate::catalog::NoNaming)
    }

    /// Open the discovered source as a library with `naming`.
    pub fn open_with(&self, naming: &dyn Naming) -> Result<Library<'static>> {
        match &self.found {
            Some((kind, path)) => open_with(*kind, path, naming),
            None => Err(Error::io("no game data found: pass -i PATH or set BOF3_GAME_DIR")),
        }
    }
}

/// Open a known source with ids and archive names only.
pub fn open(kind: SourceKind, path: &Path) -> Result<Library<'static>> {
    open_with(kind, path, &crate::catalog::NoNaming)
}

/// Open a known source with `naming`.
pub fn open_with(kind: SourceKind, path: &Path, naming: &dyn Naming) -> Result<Library<'static>> {
    match kind {
        SourceKind::Folder => Library::open_with(FolderSource::open(path)?, naming),
        SourceKind::Disc => Library::open_with(DiscSource::open(path)?, naming),
    }
}

fn folder_ok(dir: &Path) -> std::result::Result<String, String> {
    let source = FolderSource::open(dir).map_err(|e| e.message().to_string())?;
    let count = source
        .list()
        .map_err(|e| e.message().to_string())?
        .iter()
        .filter(|p| p.to_ascii_lowercase().starts_with("bin/bgm/") && p.to_ascii_lowercase().ends_with(".emi"))
        .count();
    if count == 0 { Err("no BIN/BGM/*.EMI archive".into()) } else { Ok(format!("BIN/BGM: {count} EMI archives")) }
}

fn probe(path: &Path) -> (Option<SourceKind>, PathBuf, String) {
    if !path.exists() {
        return (None, path.to_path_buf(), "does not exist".into());
    }
    let mut reason = String::new();
    let is_dir = path.is_dir();
    if is_dir || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("emi")) {
        let mut dir = if is_dir { path.to_path_buf() } else { path.parent().unwrap_or(Path::new(".")).to_path_buf() };
        for ascent in 0..=ROOT_ASCENT {
            match folder_ok(&dir) {
                Ok(found) => {
                    let up = if ascent > 0 { format!(" ({ascent} level(s) up from the hint)") } else { String::new() };
                    return (Some(SourceKind::Folder), dir, format!("{found}{up}"));
                }
                Err(why) if ascent == 0 => reason = why,
                Err(_) => {}
            }
            match dir.parent() {
                Some(parent) if parent != dir && !parent.as_os_str().is_empty() => dir = parent.to_path_buf(),
                _ => break,
            }
        }
    }
    match DiscSource::open(path) {
        Ok(disc) => match disc.list() {
            Ok(files) if files.iter().any(|f| f.to_ascii_lowercase().starts_with("bin/")) => {
                (Some(SourceKind::Disc), path.to_path_buf(), disc.describe())
            }
            _ => (None, path.to_path_buf(), "ISO9660 root has no BIN directory".into()),
        },
        Err(e) => (None, path.to_path_buf(), if reason.is_empty() { e.message().to_string() } else { reason }),
    }
}

fn probes(hint: Option<&Path>) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut root = |label: &str, value: PathBuf| {
        out.push((label.to_string(), value.clone()));
        out.push((format!("{label}/out/extracted"), value.join("out/extracted")));
    };
    if let Some(hint) = hint {
        root("hint", hint.to_path_buf());
    }
    if let Some(value) = std::env::var_os("BOF3_GAME_DIR").filter(|v| !v.is_empty()) {
        root("BOF3_GAME_DIR", PathBuf::from(value));
    }
    let scan = |out: &mut Vec<(String, PathBuf)>, label: String, base: &Path| {
        for (suffix, sub) in [
            ("/out/extracted", "out/extracted"),
            ("/rebof3/build/disc", "rebof3/build/disc"),
            ("/BIN", "BIN"),
            ("/inputs/external", "inputs/external"),
            ("", ""),
        ] {
            out.push((format!("{label}{suffix}"), if sub.is_empty() { base.to_path_buf() } else { base.join(sub) }));
        }
    };
    if let Ok(cwd) = std::env::current_dir() {
        for (depth, base) in cwd.ancestors().take(WORKING_PARENTS + 1).enumerate() {
            let label = if depth == 0 { "cwd".into() } else { format!("cwd/{}", vec![".."; depth].join("/")) };
            scan(&mut out, label, base);
        }
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        for (depth, base) in exe.ancestors().take(EXECUTABLE_PARENTS + 1).enumerate() {
            let label = if depth == 0 { "exe".into() } else { format!("exe/{}", vec![".."; depth].join("/")) };
            scan(&mut out, label, base);
        }
    }
    out
}

/// Probe for game data. An explicit `hint` that does not exist is an error: no
/// automatic probes run then, so a typo is never silently replaced.
#[must_use]
pub fn discover(hint: Option<&Path>) -> Discovery {
    let mut candidates = Vec::new();
    let mut list = probes(hint);
    if hint.is_some_and(|h| !h.exists()) {
        list.truncate(2);
    }
    for (label, path) in list {
        let (kind, path, detail) = probe(&path);
        candidates.push(Candidate { label, path: path.clone(), accepted: kind.is_some(), detail });
        if let Some(kind) = kind {
            return Discovery { candidates, found: Some((kind, path)) };
        }
    }
    Discovery { candidates, found: None }
}

/// One entry of a folder listing for choosing game data.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The file or folder name.
    pub name: String,
    /// The full path.
    pub path: PathBuf,
    /// Whether the entry is a folder. Files are disc images or cue sheets.
    pub is_dir: bool,
}

/// Where to start choosing game data: the home folder, else the working directory.
#[must_use]
pub fn start_dir() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|v| !v.is_empty());
    home.map(PathBuf::from).or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."))
}

/// The folders and the supported files (`.cue`, `.bin`, `.iso`, `.img`) in `dir`, folders first,
/// each group by name. Hidden entries are left out.
pub fn browse(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            let path = item.path();
            let is_dir = path.is_dir();
            let supported = path
                .extension()
                .is_some_and(|ext| ["cue", "bin", "iso", "img"].iter().any(|wanted| ext.eq_ignore_ascii_case(wanted)));
            (!name.starts_with('.') && (is_dir || supported)).then_some(Entry { name, path, is_dir })
        })
        .collect();
    entries.sort_by_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
    Ok(entries)
}
