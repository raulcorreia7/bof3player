// SPDX-License-Identifier: MIT
//! The entry point: a source, its catalog, and players for its items.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::catalog::{Catalog, Item, Kind, Naming, NoNaming};
use crate::driver::{MusicEngine, SampleEngine};
use crate::error::{Error, Result};
use crate::format::emi::{Emi, EntryType};
use crate::format::sep::Package;
use crate::player::{Player, XaStream};
use crate::source::Source;
use crate::spu::ReverbModel;

/// Per-player settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlayerOptions {
    /// Reverb delay-line timing for music and samples. The default is the
    /// hardware timing; [`ReverbModel::Legacy`] reproduces bof3player 0.x renders.
    pub reverb: ReverbModel,
}

/// A game's audio: its [`Source`] and the [`Catalog`] built from it.
///
/// `Library` is `Send + Sync`; any number of [`Player`]s may be created from
/// it, each used from one thread at a time.
pub struct Library<'a> {
    source: Box<dyn Source + 'a>,
    catalog: Catalog,
}

impl core::fmt::Debug for Library<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Library").field("source", &self.source.describe()).field("items", &self.catalog.len()).finish()
    }
}

impl<'a> Library<'a> {
    /// Scan `source` and build the catalog with ids and archive names only.
    pub fn open(source: impl Source + 'a) -> Result<Self> {
        Self::open_with(source, &NoNaming)
    }

    /// Scan `source` and build the catalog with `naming`.
    ///
    /// Pass [`NoNaming`] for ids and archive names
    /// only, or an implementation of [`Naming`] with the embedder's own tables.
    ///
    /// # Examples
    ///
    /// ```
    /// use bof3player::{Library, MemorySource, NoNaming};
    ///
    /// let library = Library::open_with(MemorySource::new(), &NoNaming)?;
    /// assert!(library.catalog().is_empty());
    /// # Ok::<(), bof3player::Error>(())
    /// ```
    pub fn open_with(source: impl Source + 'a, naming: &dyn Naming) -> Result<Self> {
        let source: Box<dyn Source + 'a> = Box::new(source);
        let catalog = Catalog::build(source.as_ref(), naming)?;
        Ok(Self { source, catalog })
    }

    /// The items of this library.
    #[must_use]
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Create a player positioned at the start of `item`.
    pub fn player(&self, item: &Item) -> Result<Player<'_>> {
        self.player_with(item, PlayerOptions::default())
    }

    /// Create a player with explicit [`PlayerOptions`].
    pub fn player_with(&self, item: &Item, options: PlayerOptions) -> Result<Player<'_>> {
        if !item.playable {
            return Err(Error::State(format!("{} carries no audio - {}", item.id, item.detail)));
        }
        match item.kind {
            Kind::Music => {
                let blob = self.source.open(&item.path)?;
                let engine = MusicEngine::new(&blob.bytes()?, item.index, options.reverb)?;
                Ok(Player::new(engine))
            }
            Kind::Sample => {
                let blob = self.source.open(&item.path)?;
                let engine = SampleEngine::new(&blob.bytes()?, item.bank as usize, item.sub_index, options.reverb)?;
                Ok(Player::new(engine))
            }
            Kind::Bank => {
                Err(Error::Unsupported(String::from("bank audition is not available; play its samples or music")))
            }
            Kind::Xa => {
                let blob = self.source.open(&item.path)?;
                let stream = XaStream {
                    channel: item.index as u8,
                    first_sector: item.sub_index,
                    sector_count: item.sectors,
                    duration: item.duration.unwrap_or(0.0),
                };
                Ok(Player::xa(blob, stream)?)
            }
            Kind::Cdda => {
                let blob = self.source.open(&item.path)?;
                let raw = item.path.to_ascii_lowercase().ends_with(".raw");
                Ok(Player::cdda(blob, raw)?)
            }
        }
    }

    /// A music item's sequence as a Standard MIDI File.
    pub fn export_midi(&self, item: &Item) -> Result<Vec<u8>> {
        if item.kind != Kind::Music {
            return Err(Error::invalid(format!("{} is not a music item", item.id)));
        }
        let blob = self.source.open(&item.path)?;
        let bytes = blob.bytes()?;
        let emi = Emi::parse(&bytes)?;
        Ok(Package::parse(emi.find(EntryType::Seq, 0)?)?.to_midi(item.index as usize)?)
    }

    /// The raw ADPCM blocks of a sample item (the body of a `.VAG` without its header).
    pub fn export_vag(&self, item: &Item) -> Result<Vec<u8>> {
        if item.kind != Kind::Sample {
            return Err(Error::invalid(format!("{} is not a sample item", item.id)));
        }
        let blob = self.source.open(&item.path)?;
        let bytes = blob.bytes()?;
        let emi = Emi::parse(&bytes)?;
        let body = emi.find(EntryType::Vb, item.bank as usize)?;
        let vab = crate::format::vab::Bank::parse(emi.find(EntryType::Vh, item.bank as usize)?, body)?;
        Ok(vab.vag_bytes(body, item.sub_index)?.to_vec())
    }

    /// The sequence package of a music item: the bytes the disc stores, the
    /// `.seq` file.
    pub fn export_sequence(&self, item: &Item) -> Result<Vec<u8>> {
        if item.kind != Kind::Music {
            return Err(Error::invalid(format!("{} is not a music item", item.id)));
        }
        let blob = self.source.open(&item.path)?;
        let bytes = blob.bytes()?;
        let emi = Emi::parse(&bytes)?;
        Ok(emi.find(EntryType::Seq, 0)?.to_vec())
    }

    /// The whole sound bank of a bank item: the `.vh` header followed by the
    /// `.vb` body, which is the standard VAB file.
    pub fn export_bank(&self, item: &Item) -> Result<Vec<u8>> {
        if item.kind != Kind::Bank {
            return Err(Error::invalid(format!("{} is not a bank item", item.id)));
        }
        let blob = self.source.open(&item.path)?;
        let bytes = blob.bytes()?;
        let emi = Emi::parse(&bytes)?;
        let mut bank = emi.find(EntryType::Vh, item.bank as usize)?.to_vec();
        bank.extend_from_slice(emi.find(EntryType::Vb, item.bank as usize)?);
        Ok(bank)
    }
}
