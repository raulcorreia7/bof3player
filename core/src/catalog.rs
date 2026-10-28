// SPDX-License-Identifier: MIT
//! The catalog: every item a source holds, with stable ids and ranked search.
//!
//! Ids are stable and human-typable. A consumer never parses them; it passes
//! them back to [`Catalog::find`].
//!
//! | Id | What it is |
//! |---|---|
//! | `music:BGM000#0` | sequence 0 of `BIN/BGM/BGM000.EMI`, played with that archive's bank |
//! | `bank:BOSS001`, `bank:X@2` | a VAB soundfont (the second bank of an archive is `@2`) |
//! | `bank:BOSS001#cue3` | record 3 of the archive's own cue list |
//! | `sample:BOSS001#3`, `sample:X@2#3` | one VAG inside a bank |
//! | `xa:S_XA00#ch3` | a whole channel of an XA stream |
//! | `xa:MAGIC00#ch9@12665` | one cue of that channel: the game's own cue 0x1211 |
//! | `xa:S_XA00#ch3@6192` | the EOF-delimited span of a stream with no cue table |
//! | `cdda:track02` | the CD-DA track |

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::clock;
use crate::error::{Error, Result};
use crate::format::emi::{self as emi_format, Emi, EntryType};
use crate::format::sep::{self, Package};
use crate::format::vab::{Bank, Tone, Vag};
use crate::format::wav;
use crate::format::xa;
use crate::source::{Blob, Source};
use crate::xa_cues::{self, CueTable};
use emi::cue::CueTable as BankCues;
use psx_formats::adpcm::{BLOCK_BYTES, BLOCK_SAMPLES};

const MICROSECONDS_PER_MINUTE: u32 = 60_000_000;
const PAN_CENTRE: i32 = 64;
const PAN_MAX: i32 = 127;
const CDDA_RATE: u32 = 44_100;
const CDDA_SECTOR_BYTES: u64 = 2352;
const CDDA_FRAMES_PER_SECTOR: u64 = 588;
const SILENCE_WINDOW: usize = 64 * 1024;
const WAV_HEAD_BYTES: usize = 64 * 1024;
const XA_SCAN_WINDOW_SECTORS: usize = 256;

/// The kind of media an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A sequence played through its bank.
    Music,
    /// A VAB soundfont or one of its cue records.
    Bank,
    /// One raw sample of a bank.
    Sample,
    /// Streaming XA audio.
    Xa,
    /// A CD-DA track.
    Cdda,
}

impl Kind {
    /// All kinds, in catalog order.
    pub const ALL: [Kind; 5] = [Kind::Music, Kind::Bank, Kind::Sample, Kind::Xa, Kind::Cdda];

    /// The kinds in the order the browsers list them.
    pub const LIST: [Kind; 5] = [Kind::Music, Kind::Sample, Kind::Xa, Kind::Cdda, Kind::Bank];

    /// The heading the browsers show for the kind.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Kind::Music => "Music",
            Kind::Sample => "Sound effects",
            Kind::Xa => "Streamed audio",
            Kind::Cdda => "CD audio",
            Kind::Bank => "Sound banks",
        }
    }

    /// The id prefix: `music`, `bank`, `sample`, `xa`, `cdda`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Kind::Music => "music",
            Kind::Bank => "bank",
            Kind::Sample => "sample",
            Kind::Xa => "xa",
            Kind::Cdda => "cdda",
        }
    }

    /// Parse an id prefix.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(name))
    }
}

/// One catalog entry.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Item {
    /// Media kind.
    pub kind: Kind,
    /// Stable id, for example `music:BGM000#0`.
    pub id: String,
    /// Display name, for example `BGM000 #0`.
    pub name: String,
    /// Soundtrack title, empty when none is known.
    pub title: String,
    /// The item this one copies byte for byte, when they differ only by name.
    pub same_as: Option<String>,
    /// Group the item belongs to (`BGM`, `BOSS`, `SCE_XA`, `CDDA`, ...).
    pub family: String,
    /// One-line summary.
    pub detail: String,
    /// Source file the item is read from.
    pub path: String,
    /// File-level index: sequence id, bank number or channel.
    pub index: u32,
    /// Sample number or first sector.
    pub sub_index: u32,
    /// Length in seconds when known without rendering.
    pub duration: Option<f64>,
    /// Whether the item has audio to play. Unused sequence slots, silent samples
    /// and empty channels are not playable.
    pub playable: bool,
    /// For banks, cues and samples: the archive's 0-based bank number.
    pub bank: u32,
    /// A bank item that is one record of the archive's cue list.
    pub cue: bool,
    /// For XA items: the channel's sectors inside this item's range.
    pub sectors: u32,
}

impl Item {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind,
            id: String::new(),
            name: String::new(),
            title: String::new(),
            same_as: None,
            family: String::new(),
            detail: String::new(),
            path: String::new(),
            index: 0,
            sub_index: 0,
            duration: None,
            playable: false,
            bank: 0,
            cue: false,
            sectors: 0,
        }
    }
}

/// How well an item matches a query; higher is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rank {
    /// No match.
    None,
    /// The query's characters appear in order.
    Subsequence,
    /// The query appears whole.
    Substring,
    /// The name or title starts with the query.
    Prefix,
    /// The query is the item's id.
    ExactId,
}

/// Restrictions for [`Catalog::search`].
#[derive(Debug, Clone, Default)]
pub struct Filter<'a> {
    /// Only these kinds; empty means every kind.
    pub kinds: Vec<Kind>,
    /// Only this family (ASCII case-insensitive).
    pub family: Option<&'a str>,
    /// Drop items without audio.
    pub playable_only: bool,
}

/// The game names and titles the catalog shows for its items.
///
/// The catalog itself knows ids, paths and formats only. Its builder asks this
/// adapter for names. [`Library::open`](crate::Library::open) supplies none;
/// [`Library::open_with`](crate::Library::open_with) takes any implementation,
/// including [`NoNaming`].
///
/// # Examples
///
/// ```
/// use bof3player::catalog::Naming;
///
/// struct MyNames;
///
/// impl Naming for MyNames {
///     fn music_title(&self, archive: &str, sequence: u32) -> Option<&str> {
///         (archive == "BGM000" && sequence == 0).then_some("My title")
///     }
/// }
///
/// assert_eq!(MyNames.music_title("BGM000", 0), Some("My title"));
/// assert_eq!(MyNames.music_title("BGM000", 1), None);
/// ```
pub trait Naming {
    /// The name of one `xa:` item, when the game records one.
    fn xa_span(&self, _id: &str) -> Option<&str> {
        None
    }

    /// The `ARCHIVE#sequence` slot whose sequence data this slot repeats.
    fn music_copy(&self, _slot: &str) -> Option<&str> {
        None
    }

    /// The game's sound id for an `ARCHIVE#sequence` slot.
    fn music_sound(&self, _slot: &str) -> Option<u16> {
        None
    }

    /// The name of the bank at an archive path.
    fn bank_name(&self, _path: &str) -> Option<&str> {
        None
    }

    /// The first archive of the identical-content group the archive belongs to.
    fn bank_duplicate(&self, _archive: &str) -> Option<&str> {
        None
    }

    /// The ability name of an ability index.
    fn magic_ability(&self, _ability: u16) -> Option<&str> {
        None
    }

    /// The soundtrack title of one sequence of an archive.
    fn music_title(&self, _archive: &str, _sequence: u32) -> Option<&str> {
        None
    }
}

/// A [`Naming`] that supplies no game names: ids, archive names and details
/// only.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNaming;

impl Naming for NoNaming {}

/// Every item of one source, in one flat list ordered by kind then natural id order.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    items: Vec<Item>,
    by_id: BTreeMap<String, usize>,
}

fn natural_compare(left: &str, right: &str) -> Ordering {
    let (l, r) = (left.as_bytes(), right.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < l.len() && j < r.len() {
        if l[i].is_ascii_digit() && r[j].is_ascii_digit() {
            let ls = i;
            while i < l.len() && l[i] == b'0' {
                i += 1;
            }
            let rs = j;
            while j < r.len() && r[j] == b'0' {
                j += 1;
            }
            let (ln, rn) = (i, j);
            while i < l.len() && l[i].is_ascii_digit() {
                i += 1;
            }
            while j < r.len() && r[j].is_ascii_digit() {
                j += 1;
            }
            let order =
                (i - ln).cmp(&(j - rn)).then_with(|| l[ln..i].cmp(&r[rn..j])).then_with(|| (ln - ls).cmp(&(rn - rs)));
            if order != Ordering::Equal {
                return order;
            }
            continue;
        }
        if l[i] != r[j] {
            return l[i].cmp(&r[j]);
        }
        i += 1;
        j += 1;
    }
    (l.len() - i).cmp(&(r.len() - j))
}

fn contains_ci(text: &str, needle: &str) -> bool {
    text.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

fn covers_ci(text: &str, needle: &str) -> bool {
    let mut wanted = needle.chars().map(|c| c.to_ascii_lowercase());
    let mut next = wanted.next();
    for c in text.chars() {
        match next {
            Some(w) if w == c.to_ascii_lowercase() => next = wanted.next(),
            Some(_) => {}
            None => break,
        }
    }
    next.is_none()
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len() && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

impl Catalog {
    /// Enumerate every item of `source`. A file that announces itself as media
    /// but cannot be parsed fails the build with its path in the message.
    pub(crate) fn build(source: &dyn Source, naming: &dyn Naming) -> Result<Self> {
        let mut items = Vec::with_capacity(24 * 1024);
        let cues = CueTable::new(source, naming);
        for path in source.list()? {
            add_path(&mut items, source, &cues, &path, naming)?;
        }
        items.sort_by(|a, b| {
            a.kind.cmp(&b.kind).then_with(|| natural_compare(&a.id, &b.id)).then_with(|| a.id.cmp(&b.id))
        });
        for pair in items.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(Error::format(format!("duplicate catalog id: {}", pair[0].id)));
            }
        }
        let by_id = items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        Ok(Self { items, by_id })
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// How many items have audio to play.
    #[must_use]
    pub fn playable_count(&self) -> usize {
        self.items.iter().filter(|item| item.playable).count()
    }

    /// Whether the catalog is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// All items.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The item at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Item> {
        self.items.get(index)
    }

    /// What `item` repeats, for display apart from its title: the other item's id without the kind
    /// prefix (`BGMBAT00#0`), then its title in parentheses when that differs from the item's own.
    #[must_use]
    pub fn same_as_text(&self, item: &Item) -> Option<String> {
        let other = item.same_as.as_deref()?;
        let short = other.split_once(':').map_or(other, |(_, rest)| rest);
        let other_id = if other.contains(':') { other.to_string() } else { format!("{}:{other}", item.kind.name()) };
        let title = self.find(&other_id).map_or("", |o| o.title.as_str());
        Some(if title.is_empty() || title == item.title { short.to_string() } else { format!("{short} ({title})") })
    }

    /// The item with this exact id (ASCII case-insensitive fallback).
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&Item> {
        self.index_of(id).map(|i| &self.items[i])
    }

    /// Position of the item with this id.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied().or_else(|| self.items.iter().position(|i| i.id.eq_ignore_ascii_case(id)))
    }

    /// How well `item` matches `query`.
    #[must_use]
    pub(crate) fn rank(item: &Item, query: &str) -> Rank {
        if query.is_empty() {
            return Rank::Subsequence;
        }
        if item.id.eq_ignore_ascii_case(query) {
            Rank::ExactId
        } else if starts_with_ci(&item.name, query) || starts_with_ci(&item.title, query) {
            Rank::Prefix
        } else if contains_ci(&item.name, query) || contains_ci(&item.id, query) || contains_ci(&item.title, query) {
            Rank::Substring
        } else if covers_ci(&item.name, query) || covers_ci(&item.id, query) {
            Rank::Subsequence
        } else {
            Rank::None
        }
    }

    /// Ranked search: exact id, then name prefix, then substring, then
    /// subsequence; equal ranks keep catalog order. An empty query matches everything.
    #[must_use]
    pub fn search(&self, query: &str, filter: &Filter<'_>) -> Vec<&Item> {
        let mut hits: Vec<(Rank, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !filter.playable_only || item.playable)
            .filter(|(_, item)| filter.kinds.is_empty() || filter.kinds.contains(&item.kind))
            .filter(|(_, item)| filter.family.is_none_or(|f| item.family.eq_ignore_ascii_case(f)))
            .map(|(i, item)| (Self::rank(item, query), i))
            .filter(|(rank, _)| *rank != Rank::None)
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.into_iter().map(|(_, i)| &self.items[i]).collect()
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn stem_of(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn directory_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| file_name(dir))
}

fn has_extension(path: &str, extension: &str) -> bool {
    file_name(path).rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case(extension))
}

fn seconds_text(seconds: f64) -> String {
    format!("{seconds:.2} s")
}

fn rate_text(rate: u32, channels: u32) -> String {
    format!("{rate} Hz {}", if channels >= 2 { "stereo" } else { "mono" })
}

fn add_path(
    items: &mut Vec<Item>,
    source: &dyn Source,
    cues: &CueTable,
    path: &str,
    naming: &dyn Naming,
) -> Result<()> {
    if has_extension(path, "EMI") {
        add_archive(items, source, path, naming).map_err(|e| e.context(path))
    } else if has_extension(path, "STR") {
        add_xa(items, source, cues, path, naming).map_err(|e| e.context(path))
    } else if directory_of(path).eq_ignore_ascii_case("CDDA")
        || (starts_with_ci(stem_of(path), "track") && (has_extension(path, "wav") || has_extension(path, "raw")))
    {
        add_cdda(items, source, path).map_err(|e| e.context(path))
    } else {
        Ok(())
    }
}

/// An archive being catalogued.
struct Archive<'a> {
    name: String,
    family: String,
    path: &'a str,
    banks: u32,
}

/// VAG data after the two-byte header is silent when every decoded block carries zero samples.
fn vag_pcm_silent(body: &[u8], vag: &Vag) -> bool {
    let (offset, size) = (vag.offset as usize, vag.size as usize);
    if size < BLOCK_BYTES || size % BLOCK_BYTES != 0 || offset > body.len() || size > body.len() - offset {
        return false;
    }
    for block in body[offset..offset + size].as_chunks::<BLOCK_BYTES>().0 {
        if block[2..].iter().any(|&b| b != 0) {
            return false;
        }
        if block[1] & 1 != 0 {
            break;
        }
    }
    true
}

struct BankTones {
    named: Vec<bool>,
    audible: Vec<bool>,
    pan_of: Vec<i32>,
}

fn bank_tones(vab: &Bank, tones_of_bank: &[Tone]) -> BankTones {
    let n = vab.vags.len();
    let mut tones =
        BankTones { named: alloc::vec![false; n], audible: alloc::vec![false; n], pan_of: alloc::vec![PAN_CENTRE; n] };
    for tone in tones_of_bank {
        let v = tone.vag as usize;
        if v < 1 || v >= n {
            continue;
        }
        let first = !tones.named[v];
        tones.named[v] = true;
        if tone.vol != 0 {
            tones.audible[v] = true;
        }
        if first {
            let pan = i32::from(tone.pan)
                + (i32::from(tone.program_pan) - PAN_CENTRE)
                + (i32::from(vab.master_pan) - PAN_CENTRE);
            tones.pan_of[v] = pan.clamp(0, PAN_MAX);
        }
    }
    tones
}

fn add_archive(items: &mut Vec<Item>, source: &dyn Source, path: &str, naming: &dyn Naming) -> Result<()> {
    let blob = source.open(path)?;
    let bytes = blob.bytes()?;
    let emi = Emi::parse(&bytes)?;
    let mut archive =
        Archive { name: stem_of(path).to_string(), family: directory_of(path).to_string(), path, banks: 0 };
    let bodies: Vec<&[u8]> = emi.payloads(EntryType::Vb).collect();
    let cue_lists: Vec<&[u8]> = emi.payloads(EntryType::Aux).collect();
    archive.banks = emi.payloads(EntryType::Vh).count() as u32;

    let mut bank_programs = 0u128;
    for (number, header) in emi.payloads(EntryType::Vh).enumerate() {
        let body = bodies.get(number).copied().unwrap_or(&[]);
        let cues = cue_lists.get(number).copied().unwrap_or(&[]);
        bank_programs |= add_bank(items, &archive, number as u32, header, body, cues, naming)?;
    }
    for payload in emi.payloads(EntryType::Seq) {
        add_sequences(items, &archive, payload, bank_programs, naming)?;
    }
    Ok(())
}

fn add_sequences(
    items: &mut Vec<Item>,
    archive: &Archive<'_>,
    payload: &[u8],
    bank_programs: u128,
    naming: &dyn Naming,
) -> Result<()> {
    let sep = Package::parse(payload)?;
    let count = sep.sequences.len();
    for sequence in &sep.sequences {
        let used = sequence.programs_used();
        let empty = !sequence.has_notes();
        let unreachable = !empty && used & bank_programs == 0;
        let mut item = Item::new(Kind::Music);
        item.id = format!("music:{}#{}", archive.name, sequence.seq_id);
        item.name = format!("{} #{}", archive.name, sequence.seq_id);
        let slot = format!("{}#{}", archive.name, sequence.seq_id);
        item.title = naming.music_title(&archive.name, u32::from(sequence.seq_id)).unwrap_or("").to_string();
        item.same_as = naming.music_copy(&slot).map(String::from);
        item.family = archive.family.clone();
        item.detail = if empty {
            "empty (no events)".to_string()
        } else if unreachable {
            format!("silent (no tone for program {})", sep::lowest_program(used))
        } else {
            let bpm = MICROSECONDS_PER_MINUTE.checked_div(sequence.tempo_us).unwrap_or(0);
            format!("{count} sequences, tempo {bpm} bpm, {} ppqn", sequence.resolution)
        };
        if let Some(id) = naming.music_sound(&slot) {
            item.detail = format!("{}; game sound id {id}", item.detail);
        }
        item.path = archive.path.to_string();
        item.index = u32::from(sequence.seq_id);
        item.duration = Some(if empty || unreachable { 0.0 } else { clock::seconds(sequence) });
        item.playable = !(empty || unreachable);
        items.push(item);
    }
    Ok(())
}

fn loop_text(vag: &Vag) -> String {
    if !vag.has_loop {
        return String::new();
    }
    let rate = f64::from(crate::format::VAB_SAMPLE_RATE);
    format!(
        ", loops {} to {}",
        seconds_text(f64::from(vag.loop_start_frame) / rate),
        seconds_text(f64::from(vag.loop_end_frame) / rate)
    )
}

fn add_bank(
    items: &mut Vec<Item>,
    archive: &Archive<'_>,
    number: u32,
    header: &[u8],
    body: &[u8],
    cues: &[u8],
    naming: &dyn Naming,
) -> Result<u128> {
    let vab = Bank::parse(header, body)?;
    let (qualifier, label) = if number == 0 {
        (String::new(), archive.name.clone())
    } else {
        (format!("@{}", number + 1), format!("{} bank {}", archive.name, number + 1))
    };
    let usable: Vec<Tone> = vab.usable_tones().copied().collect();
    let tones = bank_tones(&vab, &usable);

    let mut bank = Item::new(Kind::Bank);
    bank.id = format!("bank:{}{qualifier}", archive.name);
    bank.name = label.clone();
    if let Some(name) = naming.bank_name(archive.path) {
        bank.name = if number == 0 { name.to_string() } else { format!("{name} bank {}", number + 1) };
    }
    bank.same_as = naming.bank_duplicate(&archive.name).map(String::from);
    bank.family = archive.family.clone();
    bank.path = archive.path.to_string();
    bank.index = number;
    bank.bank = number;
    bank.playable = (1..vab.vags.len()).any(|i| tones.audible[i] && !vab.vags[i].is_empty());
    let any_volume = usable.iter().any(|t| t.vol != 0);
    let silent_note = if bank.playable {
        ""
    } else if any_volume {
        " (silent: no tone reaches a non-empty sample)"
    } else {
        " (silent: every tone volume is 0)"
    };
    bank.detail = format!(
        "audition: {} programs, {} tones, one at a time (the game keys one tone per sound){silent_note}",
        vab.program_count, vab.declared_tones
    );
    items.push(bank);

    // One sample item per non-empty VAG.
    for vag in vab.vags.iter().filter(|v| !v.is_empty()) {
        let frames = vag.size / BLOCK_BYTES as u32 * BLOCK_SAMPLES as u32;
        let seconds = f64::from(frames) / f64::from(crate::format::VAB_SAMPLE_RATE);
        let i = vag.index as usize;
        let named_silent = tones.named[i] && !tones.audible[i];
        let pcm_silent = !named_silent && vag_pcm_silent(body, vag);
        let verdict = if named_silent {
            " (silent)"
        } else if pcm_silent {
            " (silent: PCM is all zero)"
        } else if tones.pan_of[i] != PAN_CENTRE {
            " (panned in the data)"
        } else {
            ""
        };
        let mut sample = Item::new(Kind::Sample);
        sample.id = format!("sample:{}{qualifier}#{}", archive.name, vag.index);
        sample.name = format!("{label} #{}", vag.index);
        sample.family = archive.family.clone();
        sample.detail =
            format!("{} bytes, {frames} samples, {}{}{verdict}", vag.size, seconds_text(seconds), loop_text(vag));
        sample.path = archive.path.to_string();
        sample.index = number;
        sample.sub_index = vag.index;
        sample.duration = Some(seconds);
        sample.playable = !(named_silent || pcm_silent);
        sample.bank = number;
        items.push(sample);
    }

    // The archive's own cue list.
    for (i, cue) in BankCues::decode(cues).map_err(emi_format::error)?.records().iter().enumerate() {
        let raw = cue.encode();
        let mut item = Item::new(Kind::Bank);
        item.id = format!("bank:{}{qualifier}#cue{i}", archive.name);
        item.name = format!("{label} cue {i}");
        item.family = archive.family.clone();
        item.detail = format!(
            "audition: the archive's own type-8 cue list, record {i} ({:02x}{:02x} {:02x}{:02x}), one tone keyed once: program {}, tone {}",
            raw[0], raw[1], raw[2], raw[3], cue.program, cue.tone
        );
        item.path = archive.path.to_string();
        item.index = number;
        item.sub_index = i as u32;
        item.bank = number;
        item.cue = true;
        if archive.banks != 1 {
            item.detail += &format!(
                ", one of the archive's {} banks - the archive does not say which one this list was loaded with",
                archive.banks
            );
        } else if let Some(tone) = usable.iter().find(|t| t.storage_block == cue.program && t.tone_slot == cue.tone) {
            let vag_index = tone.vag as usize;
            let vag = &vab.vags[vag_index];
            let empty = vag.is_empty();
            let silent = !empty && ((tones.named[vag_index] && !tones.audible[vag_index]) || vag_pcm_silent(body, vag));
            let verdict = if empty {
                " (the sample is empty)"
            } else if silent {
                " (the sample is silent)"
            } else {
                ""
            };
            item.detail += &format!(", plays sample:{}{qualifier}#{vag_index}{verdict}", archive.name);
            item.playable = !(empty || silent);
        } else {
            item.detail += ", the bank has no tone there";
        }
        items.push(item);
    }
    Ok(vab.program_mask())
}

fn add_xa(items: &mut Vec<Item>, source: &dyn Source, cues: &CueTable, path: &str, naming: &dyn Naming) -> Result<()> {
    let blob = source.open(path)?;
    let mut scanner = xa::Scanner::new();
    let total = blob.len() / xa::SECTOR_BYTES as u64;
    let mut window = alloc::vec![0u8; XA_SCAN_WINDOW_SECTORS * xa::SECTOR_BYTES];
    let mut sector = 0u64;
    while sector < total {
        let count = (total - sector).min(XA_SCAN_WINDOW_SECTORS as u64) as usize;
        let bytes = &mut window[..count * xa::SECTOR_BYTES];
        blob.read_exact_at(sector * xa::SECTOR_BYTES as u64, bytes)?;
        for s in bytes.as_chunks::<{ xa::SECTOR_BYTES }>().0 {
            scanner.push(s);
        }
        sector += count as u64;
    }
    let stream = stem_of(path);
    let family = directory_of(path);
    let channels = scanner.finish();
    let mut coding = BTreeMap::new();
    for channel in &channels {
        coding.insert(channel.channel, channel.coding);
        let id = format!("xa:{stream}#ch{}", channel.channel);
        let label = format!("{stream} ch{}", channel.channel);
        let shape = rate_text(xa::rate(channel.coding), xa::channels(channel.coding));
        let mut item = Item::new(Kind::Xa);
        item.id = id.clone();
        item.name = label.clone();
        item.family = family.to_string();
        item.detail = format!("{} sectors, {shape}, {}", channel.sectors, seconds_text(channel.seconds));
        item.path = path.to_string();
        item.index = u32::from(channel.channel);
        item.sub_index = channel.first;
        item.duration = Some(channel.seconds);
        item.playable = channel.sectors > 0;
        item.sectors = channel.sectors;
        items.push(item);
    }
    let file = xa_cues::file_index(stream);
    let cue_list = file.map_or(&[][..], |index| cues.file(index));
    if cue_list.is_empty() {
        for channel in &channels {
            for span in &channel.spans {
                let seconds = xa::duration(span.sectors, channel.coding);
                let mut clip = Item::new(Kind::Xa);
                clip.id = format!("xa:{stream}#ch{}@{}", channel.channel, span.first);
                clip.name = format!("{stream} ch{} @{}", channel.channel, span.first);
                clip.family = family.to_string();
                clip.detail = format!(
                    "sectors {}-{} ({}), {}, {} (the data's own EOF)",
                    span.first,
                    span.eof,
                    span.sectors,
                    rate_text(xa::rate(channel.coding), xa::channels(channel.coding)),
                    seconds_text(seconds)
                );
                clip.path = path.to_string();
                clip.index = u32::from(channel.channel);
                clip.sub_index = span.first;
                clip.duration = Some(seconds);
                clip.playable = span.sectors > 0;
                clip.sectors = span.sectors;
                items.push(clip);
            }
        }
    } else {
        for cue in cue_list {
            add_xa_cue(items, path, stream, family, *cue, file.unwrap_or(0), cues, &coding, total, naming);
        }
    }
    Ok(())
}

/// One cue from the game's pair list, clamped to the sectors the stream copy
/// actually holds. The game's own stop margin is `150` sectors before the
/// window end; the catalog plays the whole window so a clip reaches its own
/// boundary.
#[allow(clippy::too_many_arguments)]
fn add_xa_cue(
    items: &mut Vec<Item>,
    path: &str,
    stream: &str,
    family: &str,
    cue: xa_cues::Cue,
    file: usize,
    table: &CueTable,
    coding: &BTreeMap<u8, u8>,
    total: u64,
    naming: &dyn Naming,
) {
    let Some(&coding) = coding.get(&cue.channel) else { return };
    let first = u64::from(cue.first_sector());
    let wanted = cue.wanted_sectors();
    // Channel sectors are at `channel + period*j`; the copy holds j up to the last one.
    let available = if first < total {
        let last = (total - 1 - u64::from(cue.channel)) / u64::from(cue.period);
        (last + 1).saturating_sub(u64::from(cue.start)) as u32
    } else {
        0
    };
    let sectors = wanted.min(available);
    let seconds = xa::duration(sectors, coding);
    let shape = rate_text(xa::rate(coding), xa::channels(coding));
    let id = format!("xa:{stream}#ch{}@{}", cue.channel, first);
    let fallback =
        if file == 1 { format!("Magic 0x{:04x}", cue.game_id) } else { format!("{stream} ch{} @{first}", cue.channel) };
    let name = naming
        .xa_span(&id)
        .map(ToString::to_string)
        .or_else(|| table.name(cue.ordinal).map(|name| format!("{name} (0x{:04x})", cue.game_id)))
        .unwrap_or(fallback);
    let detail = if sectors == 0 {
        format!("periods {}-{}, not in the extracted copy (stream ends at sector {total})", cue.start, cue.end)
    } else if sectors < wanted {
        format!(
            "periods {}-{} ({} of {} sectors in the copy), {shape}, {}, game cue 0x{:04x}",
            cue.start,
            cue.end,
            sectors,
            wanted,
            seconds_text(seconds),
            cue.game_id
        )
    } else {
        let last = first + u64::from(sectors - 1) * u64::from(cue.period);
        format!(
            "periods {}-{}, sectors {first}-{last} ({sectors}), {shape}, {}, game cue 0x{:04x}",
            cue.start,
            cue.end,
            seconds_text(seconds),
            cue.game_id
        )
    };
    let mut clip = Item::new(Kind::Xa);
    clip.id = id;
    clip.name = name;
    clip.family = family.to_string();
    clip.detail = detail;
    clip.path = path.to_string();
    clip.index = u32::from(cue.channel);
    clip.sub_index = first as u32;
    clip.duration = Some(seconds);
    clip.playable = sectors > 0;
    clip.sectors = sectors;
    items.push(clip);
}

fn payload_is_silent(blob: &dyn Blob, offset: u64, bytes: u64) -> Result<bool> {
    let mut window = alloc::vec![0u8; SILENCE_WINDOW];
    let mut at = 0;
    while at < bytes {
        let want = (bytes - at).min(SILENCE_WINDOW as u64) as usize;
        let got = blob.read_at(offset + at, &mut window[..want])?;
        if got == 0 {
            return Err(Error::io("the payload ends before its declared length"));
        }
        if window[..got].iter().any(|&b| b != 0) {
            return Ok(false);
        }
        at += got as u64;
    }
    Ok(true)
}

fn add_cdda(items: &mut Vec<Item>, source: &dyn Source, path: &str) -> Result<()> {
    let blob = source.open(path)?;
    let size = blob.len();
    let mut item = Item::new(Kind::Cdda);
    item.id = format!("cdda:{}", stem_of(path).to_ascii_lowercase());
    item.name = stem_of(path).to_string();
    item.family = "CDDA".to_string();
    item.path = path.to_string();
    item.playable = true;
    let (mut payload_offset, mut payload_bytes) = (0, size);
    let shape = if has_extension(path, "wav") {
        let mut head = alloc::vec![0u8; WAV_HEAD_BYTES.min(size as usize)];
        let got = blob.read_at(0, &mut head)?;
        wav::parse(&head[..got]).ok()
    } else {
        None
    };
    if let Some(info) = shape {
        let seconds = info.frames() as f64 / f64::from(info.rate);
        item.detail = format!("{}, {}", rate_text(info.rate, info.channels), seconds_text(seconds));
        item.duration = Some(seconds);
        payload_offset = info.data_offset;
        payload_bytes = info.data_bytes;
    } else if !has_extension(path, "wav") {
        let sectors = size / CDDA_SECTOR_BYTES;
        let seconds = (sectors * CDDA_FRAMES_PER_SECTOR) as f64 / f64::from(CDDA_RATE);
        item.detail = format!("{sectors} sectors, {}, {}", rate_text(CDDA_RATE, 2), seconds_text(seconds));
        item.duration = Some(seconds);
    } else {
        item.detail = "WAV track, duration unknown".to_string();
    }
    if payload_is_silent(&*blob, payload_offset, payload_bytes).unwrap_or(false) {
        item.playable = false;
        item.detail += "; no audio in this rip (all-zero payload)";
    }
    items.push(item);
    Ok(())
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    fn item(kind: Kind, id: &str, name: &str, title: &str, family: &str, playable: bool) -> Item {
        Item {
            id: id.to_string(),
            name: name.to_string(),
            title: title.to_string(),
            family: family.to_string(),
            playable,
            ..Item::new(kind)
        }
    }

    fn catalog() -> Catalog {
        let mut items = vec![
            item(Kind::Music, "music:BGM10#0", "BGM10 #0", "Battle", "BGM", true),
            item(Kind::Music, "music:BGM2#0", "BGM2 #0", "Opening", "BGM", true),
            item(Kind::Music, "music:BGM2#1", "BGM2 #1", "", "BGM", false),
            item(Kind::Sample, "sample:BOSS001#3", "BOSS001 #3", "", "BOSS", true),
            item(Kind::Xa, "xa:S_XA00#ch0@0", "S_XA00 ch0", "", "S_XA", true),
        ];
        items.sort_by(|a, b| {
            a.kind.cmp(&b.kind).then_with(|| natural_compare(&a.id, &b.id)).then_with(|| a.id.cmp(&b.id))
        });
        let by_id = items.iter().enumerate().map(|(i, item)| (item.id.clone(), i)).collect();
        Catalog { items, by_id }
    }

    #[test]
    fn natural_order_compares_numbers_by_value() {
        assert_eq!(natural_compare("BGM2", "BGM10"), Ordering::Less);
        assert_eq!(natural_compare("BGM10", "BGM2"), Ordering::Greater);
        assert_eq!(natural_compare("BGM02", "BGM2"), Ordering::Greater);
        assert_eq!(natural_compare("A", "A"), Ordering::Equal);
        assert_eq!(natural_compare("A", "AB"), Ordering::Less);
    }

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(Kind::from_name("MUSIC"), Some(Kind::Music));
        assert_eq!(Kind::from_name("movie"), None);
    }

    #[test]
    fn items_sort_by_kind_then_natural_id() {
        let catalog = catalog();
        let ids: Vec<_> = catalog.items().iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["music:BGM2#0", "music:BGM2#1", "music:BGM10#0", "sample:BOSS001#3", "xa:S_XA00#ch0@0"]);
    }

    #[test]
    fn find_accepts_any_letter_case() {
        let catalog = catalog();
        assert_eq!(catalog.find("music:BGM2#0").map(|i| i.title.as_str()), Some("Opening"));
        assert_eq!(catalog.find("MUSIC:bgm2#0").map(|i| i.title.as_str()), Some("Opening"));
        assert!(catalog.find("music:BGM3#0").is_none());
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_substring_then_subsequence() {
        let catalog = catalog();
        let hits =
            |query: &str| catalog.search(query, &Filter::default()).iter().map(|i| i.id.clone()).collect::<Vec<_>>();
        assert_eq!(hits("music:BGM10#0"), ["music:BGM10#0"]);
        assert_eq!(hits("battle"), ["music:BGM10#0"]);
        // Prefix of the name beats a match inside the id.
        assert_eq!(hits("bgm2")[..2], ["music:BGM2#0", "music:BGM2#1"]);
        assert_eq!(hits("bss"), ["sample:BOSS001#3"]);
        assert!(hits("zzz").is_empty());
        assert_eq!(hits("").len(), 5);
    }

    #[test]
    fn filters_narrow_by_kind_family_and_audio() {
        let catalog = catalog();
        let count = |filter: Filter<'_>| catalog.search("", &filter).len();
        assert_eq!(count(Filter { kinds: vec![Kind::Music], ..Filter::default() }), 3);
        assert_eq!(count(Filter { family: Some("boss"), ..Filter::default() }), 1);
        assert_eq!(count(Filter { playable_only: true, ..Filter::default() }), 4);
    }

    #[test]
    fn same_as_text_names_the_original_and_its_title() {
        let catalog = catalog();
        let copy = Item {
            same_as: Some("music:BGM2#0".to_string()),
            ..item(Kind::Music, "music:X#0", "X", "Battle", "BGM", true)
        };
        assert_eq!(catalog.same_as_text(&copy).as_deref(), Some("BGM2#0 (Opening)"));
        let same_title = Item { title: "Opening".to_string(), ..copy.clone() };
        assert_eq!(catalog.same_as_text(&same_title).as_deref(), Some("BGM2#0"));
        assert_eq!(catalog.same_as_text(&catalog.items[0].clone()), None);
        let unknown = Item { same_as: Some("music:GONE#0".to_string()), ..copy };
        assert_eq!(catalog.same_as_text(&unknown).as_deref(), Some("GONE#0"));
    }
}
