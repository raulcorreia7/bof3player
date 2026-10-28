// SPDX-License-Identifier: MIT
//! # bof3player
//!
//! PlayStation audio for *Breath of Fire III*: VAB soundbanks, SEP sequences,
//! CD-ROM XA streams and CD-DA, rendered through an SPU-accurate voice engine.
//!
//! The engine owns the audio device and the mixing thread; this crate only
//! produces samples. Open a [`Library`] over a [`Source`], look items up in its
//! [`Catalog`], and pull PCM from a [`Player`]. The generic PlayStation audio
//! (SPU, XA and CD-DA playback, file sources, WAV/FLAC/Ogg export) lives in
//! `psx_audio` and is re-exported here.
//!
//! The catalog knows ids, paths and formats only. [`Library::open`] supplies
//! ids and archive names; [`Library::open_with`] takes an embedder's [`Naming`]
//! implementation.
//!
//! ```no_run
//! use bof3player::{Library, MemorySource};
//!
//! # fn main() -> bof3player::Result<()> {
//! let emi_bytes: &[u8] = &[]; // BIN/BGM/BGM000.EMI, loaded by the engine
//! let mut source = MemorySource::new();
//! source.add("BIN/BGM/BGM000.EMI", emi_bytes);
//! let library = Library::open(source)?;
//! let item = library.catalog().find("music:BGM000#0").expect("item exists");
//! let mut player = library.player(item)?;
//! let mut pcm = vec![0.0f32; 1024 * player.channels() as usize];
//! let frames = player.render_f32(&mut pcm)?;
//! # let _ = frames;
//! # Ok(())
//! # }
//! ```
//!
//! ## Features
//!
//! * `std` (default): folder and disc-image sources and `Read + Seek` adapters.
//!   WAV and FLAC export work without it (`no_std + alloc`).
//! * `ogg` (default): Ogg Vorbis export through the bundled libvorbis; adds a
//!   C compiler at build time.
//! * `audio` (off): the [`deck`] audio output through `cpal`.
//! * `gui` (off): the `bof3player-gui` window.
//! * `cli` (off): the `bof3player` binary and its terminal browser.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod catalog;
#[cfg(feature = "cli")]
pub mod cli;
mod clock;
#[cfg(feature = "audio")]
pub mod deck;
#[cfg(feature = "std")]
pub mod discover;
pub mod format;
#[cfg(feature = "gui")]
pub mod gui;
pub mod library;
#[cfg(feature = "names")]
pub mod names;
#[cfg(any(feature = "cli", feature = "gui"))]
pub mod options;
pub mod player;
#[cfg(feature = "std")]
pub mod save;
#[cfg(feature = "audio")]
pub mod spectrum;
pub mod theme;

mod driver;
mod xa_cues;

pub use catalog::{Catalog, Item, Kind, Naming, NoNaming};
pub use library::{Library, PlayerOptions};
pub use player::Player;
pub use psx_audio::{Blob, CallbackSource, Error, MemorySource, Result, Source, error, export, source, spu};
#[cfg(feature = "std")]
pub use psx_audio::{DiscSource, FolderSource, ReaderBlob};

/// Version of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
