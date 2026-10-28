// SPDX-License-Identifier: MIT
//! The switches the terminal player and the window share.

use std::path::PathBuf;

use clap::Args;

use crate::PlayerOptions;
use crate::spu::ReverbModel;

/// Settings every command shares.
#[derive(Args, Debug, Clone)]
pub struct Global {
    /// Game data: an extracted folder, a .cue sheet or a disc image
    /// (default: $BOF3_GAME_DIR, then the working and executable directories)
    #[arg(short = 'i', long = "input", alias = "dir", global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// Names file (JSON, see docs/player.md); overrides $BOF3_NAMES and the default
    /// bof3-names.json in the working and executable directories
    #[arg(long, global = true, value_name = "FILE")]
    pub names: Option<PathBuf>,
    /// Machine-readable output where supported (default: off)
    #[arg(long, global = true)]
    pub json: bool,
    /// Start with the sound off; the speaker button or `M` turns it on (default: off)
    #[arg(long, global = true)]
    pub mute: bool,
    /// Explain what is happening (default: off)
    #[arg(short, long, global = true)]
    pub verbose: bool,
    /// Print only results and errors: no `(n of total shown)` or `wrote` lines (default: off)
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    /// Use the earlier reverb timing (reproduces old renders; default: off)
    #[arg(long, global = true)]
    pub legacy_reverb: bool,
    /// Folder `w`, `x` and `X` write into
    #[arg(long, global = true, value_name = "DIR", default_value_os_t = crate::save::default_export_dir())]
    pub export_dir: PathBuf,
    /// Plays of a looping track's window, counting the first (default 1; FLAC/Ogg music
    /// defaults to the intro plus one loop)
    #[arg(long, global = true, value_name = "N")]
    pub loops: Option<u32>,
    /// Seconds of fade-out at the end of a saved looping track
    /// (default: 10 in the browser, none for render)
    #[arg(long, global = true, value_name = "SECONDS")]
    pub fade: Option<f64>,
}

impl Global {
    /// The folder the browser exports into.
    #[must_use]
    pub fn exports(&self) -> PathBuf {
        self.export_dir.clone()
    }

    /// The player settings these options select.
    #[must_use]
    pub fn player_options(&self) -> PlayerOptions {
        PlayerOptions {
            reverb: if self.legacy_reverb { ReverbModel::Legacy } else { ReverbModel::Hardware },
            ..PlayerOptions::default()
        }
    }
}
