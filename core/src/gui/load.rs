// SPDX-License-Identifier: MIT
//! Opening game data off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::Library;
use crate::discover::{self, SourceKind};
use crate::names;

/// Game data that opened.
pub struct Loaded {
    /// The catalog and its source. Leaked: players borrow it for `'static`.
    pub library: &'static Library<'static>,
    /// Whether the data is a disc image or a folder.
    pub kind: SourceKind,
    /// The disc image or folder that was opened.
    pub path: PathBuf,
    /// Something the user should know, such as a names file that did not read.
    pub notice: Option<String>,
}

/// Game data that did not open.
pub struct Failed {
    /// What failed.
    pub message: String,
    /// Every place that was probed and why it was rejected.
    pub probes: Vec<String>,
}

/// A load in progress. Poll it every frame.
pub struct Loading(Receiver<Result<Loaded, Failed>>);

impl Loading {
    /// Start probing `hint`, or the default places when there is none. `names` is an optional
    /// titles file.
    pub fn start(hint: Option<&Path>, names: Option<&Path>) -> Self {
        let hint = hint.map(Path::to_path_buf);
        let names = names.map(Path::to_path_buf);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // A closed receiver means a newer load replaced this one.
            let _ = tx.send(open(hint.as_deref(), names.as_deref()));
        });
        Self(rx)
    }

    /// The result, once the load is done.
    pub fn poll(&self) -> Option<Result<Loaded, Failed>> {
        self.0.try_recv().ok()
    }
}

fn open(hint: Option<&Path>, names: Option<&Path>) -> Result<Loaded, Failed> {
    let discovery = discover::discover(hint);
    let probes = discovery
        .candidates
        .iter()
        .map(|c| format!("{}: {} ({})", c.label, c.path.display(), c.detail))
        .collect::<Vec<_>>();
    let Some((kind, path)) = discovery.found.clone() else {
        let message = match hint {
            Some(hint) => format!("No game data found at {}.", hint.display()),
            None => "No game data found.".to_string(),
        };
        return Err(Failed { message, probes });
    };
    let mut notice = None;
    let (titles, failure) = names::load(names);
    if let Some(failure) = failure {
        notice = Some(format!("{failure}. Titles are not shown."));
    }
    match discovery.open_with(&titles) {
        Ok(library) => Ok(Loaded { library: Box::leak(Box::new(library)), kind, path, notice }),
        Err(error) => Err(Failed { message: format!("Cannot read {}: {}.", path.display(), error.message()), probes }),
    }
}
