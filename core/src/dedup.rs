// SPDX-License-Identifier: MIT
//! Content-based grouping for playable items.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use crate::catalog::{Item, Kind};
use crate::error::Result;
use crate::format::emi::{Emi, EntryType};
use crate::format::sep::{self, Package};
use crate::format::vab::Bank;
use crate::source::Source;

#[derive(Debug, Clone)]
/// A group of duplicate items sharing the same content fingerprint.
pub struct Group {
    /// The content fingerprint key.
    pub key: String,
    /// The canonical item id.
    pub canonical_id: String,
    /// All member item ids in the group.
    pub member_ids: Vec<String>,
    /// Number of members.
    pub count: usize,
}

#[derive(Debug, Clone, Default)]
/// Collection of content-based duplicate groups.
pub struct Groups {
    map: BTreeMap<String, Vec<String>>,
    canonical: BTreeMap<String, String>,
}

impl Groups {
    /// Create a new empty grouping.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an item to grouping with its content key.
    pub fn add(&mut self, key: String, item_id: String) {
        let entry = self.map.entry(key.clone()).or_default();
        if entry.is_empty() {
            self.canonical.insert(key.clone(), item_id.clone());
        }
        entry.push(item_id);
    }

    /// All duplicate groups with more than one member.
    #[must_use]
    pub fn groups(&self) -> Vec<Group> {
        let mut result = Vec::new();
        for (key, members) in &self.map {
            if members.len() <= 1 {
                continue;
            }
            let canonical = self.canonical.get(key).cloned().unwrap_or_else(|| members[0].clone());
            result.push(Group {
                key: key.clone(),
                canonical_id: canonical,
                member_ids: members.clone(),
                count: members.len(),
            });
        }
        result
    }

    /// Whether this item is a non-canonical duplicate in its group.
    #[must_use]
    pub fn is_duplicate(&self, item_id: &str) -> bool {
        for members in self.map.values() {
            if members.len() > 1 && members.contains(&item_id.to_string()) && members[0] != item_id {
                return true;
            }
        }
        false
    }

    /// The canonical item id for the group containing this item.
    #[must_use]
    pub fn canonical_of(&self, item_id: &str) -> Option<String> {
        for (key, members) in &self.map {
            if members.contains(&item_id.to_string()) && members.len() > 1 {
                return self.canonical.get(key).cloned();
            }
        }
        None
    }

    /// All members of the duplicate group containing this item.
    #[must_use]
    pub fn group_members(&self, item_id: &str) -> Option<Vec<String>> {
        for members in self.map.values() {
            if members.contains(&item_id.to_string()) && members.len() > 1 {
                return Some(members.clone());
            }
        }
        None
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use core::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn push_u16_be(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn push_u24_be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
}
fn push_u32_be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn write_vlq(out: &mut Vec<u8>, mut value: u32) {
    let mut buffer = [0u8; 5];
    let mut i = 4;
    buffer[i] = (value & 0x7f) as u8;
    while value > 0x7f {
        value >>= 7;
        i -= 1;
        buffer[i] = ((value & 0x7f) | 0x80) as u8;
    }
    for b in &buffer[i..=4] {
        out.push(*b);
    }
}

fn encode_events_for(sequence: &sep::Sequence) -> Vec<u8> {
    let mut out = Vec::new();
    let mut running: u8 = 0;
    for event in &sequence.events {
        write_vlq(&mut out, event.delta);
        if event.status_byte {
            if event.status != running {
                out.push(event.status);
                running = event.status;
            }
            match event.kind() {
                sep::Kind::Program => {
                    out.push(event.data1);
                }
                sep::Kind::NoteOn | sep::Kind::NoteOff | sep::Kind::PolyAftertouch => {
                    out.push(event.data1);
                    out.push(event.data2);
                }
                sep::Kind::Control => {
                    out.push(event.data1);
                    out.push(event.data2);
                }
                sep::Kind::PitchBend => {
                    out.push(event.data1);
                    out.push(event.data2);
                }
                sep::Kind::ChannelAftertouch => {
                    out.push(event.data1);
                }
                sep::Kind::Meta => {
                    out.push(event.meta_type);
                    write_vlq(&mut out, event.meta.len() as u32);
                    out.extend_from_slice(&event.meta);
                }
            }
        } else {
            match event.kind() {
                sep::Kind::Program => {
                    out.push(event.data1);
                }
                sep::Kind::NoteOn | sep::Kind::NoteOff | sep::Kind::PolyAftertouch => {
                    out.push(event.data1);
                    out.push(event.data2);
                }
                sep::Kind::Control => {
                    out.push(event.data1);
                    out.push(event.data2);
                }
                sep::Kind::PitchBend => {
                    out.push(event.data1);
                    out.push(event.data2);
                }
                sep::Kind::ChannelAftertouch => {
                    out.push(event.data1);
                }
                sep::Kind::Meta => {
                    if running != 0xff {
                        out.push(0xff);
                        running = 0xff;
                    }
                    out.push(event.meta_type);
                    write_vlq(&mut out, event.meta.len() as u32);
                    out.extend_from_slice(&event.meta);
                }
            }
        }
    }
    out
}

/// Compute the content-based grouping key for an item.
pub fn group_key(item: &Item, source: &dyn Source) -> Result<Option<String>> {
    match item.kind {
        Kind::Music => music_key(item, source),
        Kind::Sample => sample_key(item, source),
        _ => Ok(None),
    }
}

fn music_key(item: &Item, source: &dyn Source) -> Result<Option<String>> {
    let blob = source.open(&item.path)?;
    let bytes = blob.bytes()?;
    let emi = Emi::parse(&bytes)?;
    let seq_bytes = emi.find(EntryType::Seq, 0)?;
    let sep = Package::parse(seq_bytes)?;
    let selected = match sep.sequences.get(item.index as usize) {
        Some(s) => s,
        None => return Ok(None),
    };
    if !selected.has_notes() {
        return Ok(None);
    }
    let used = selected.programs_used();
    if used == 0 {
        return Ok(None);
    }
    let header = emi.find(EntryType::Vh, item.bank as usize)?;
    let body = emi.find(EntryType::Vb, item.bank as usize)?;
    let bank = Bank::parse(header, body)?;

    let mut hasher = Sha256::new();
    let events = encode_events_for(selected);
    let mut seq_block = Vec::new();
    push_u16_be(&mut seq_block, selected.seq_id);
    push_u16_be(&mut seq_block, selected.resolution);
    push_u24_be(&mut seq_block, selected.tempo_us);
    seq_block.push(selected.time_num);
    seq_block.push(selected.time_den);
    push_u32_be(&mut seq_block, events.len() as u32);
    seq_block.extend_from_slice(&events);
    hasher.update(&seq_block);

    let mut used_programs = Vec::new();
    for i in 0..128 {
        if ((used >> (i as u32)) & 1) == 1 {
            used_programs.push(i);
        }
    }
    if used_programs.is_empty() {
        return Ok(None);
    }
    used_programs.sort_unstable();
    for prog in &used_programs {
        hasher.update([*prog as u8]);
    }

    let mut relevant_tones: Vec<_> = bank
        .tones
        .iter()
        .filter(|t| t.is_usable(bank.vag_count))
        .filter(|t| {
            let p = ((t.program as u32) & 0x7f) as usize;
            p < 128 && ((used >> (p as u32)) & 1) == 1
        })
        .collect();
    relevant_tones.sort_by_key(|t| (t.storage_block, t.tone_slot, t.vag));
    for tone in &relevant_tones {
        hasher.update([tone.vag as u8]);
        hasher.update(tone.program.to_le_bytes());
        hasher.update([tone.vol, tone.pan, tone.center_note, tone.shift, tone.min_note, tone.max_note]);
        hasher.update(tone.adsr1.to_le_bytes());
        hasher.update(tone.adsr2.to_le_bytes());
        hasher.update([tone.mode, tone.priority]);
        if tone.vag >= 1
            && let Ok(vag_bytes) = bank.vag_bytes(body, tone.vag as u32)
        {
            hasher.update(vag_bytes);
        }
    }
    let result = hasher.finalize();
    Ok(Some(format!("m:{}", hex(&result))))
}

fn sample_key(item: &Item, source: &dyn Source) -> Result<Option<String>> {
    if item.kind != Kind::Sample {
        return Ok(None);
    }
    let blob = source.open(&item.path)?;
    let bytes = blob.bytes()?;
    let emi = Emi::parse(&bytes)?;
    let body = emi.find(EntryType::Vb, item.bank as usize)?;
    let header = emi.find(EntryType::Vh, item.bank as usize)?;
    let bank = Bank::parse(header, body)?;
    let mut hasher = Sha256::new();
    if let Ok(vag_bytes) = bank.vag_bytes(body, item.sub_index) {
        hasher.update(vag_bytes);
    }
    for tone in bank.tones.iter() {
        if tone.vag as u32 == item.sub_index && tone.is_usable(bank.vag_count) {
            hasher.update([tone.vol, tone.pan, tone.center_note, tone.shift]);
            hasher.update(tone.adsr1.to_le_bytes());
            hasher.update(tone.adsr2.to_le_bytes());
            break;
        }
    }
    let result = hasher.finalize();
    Ok(Some(format!("s:{}", hex(&result))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_collapse_duplicates() {
        let mut groups = Groups::new();
        groups.add("key1".to_string(), "a".to_string());
        groups.add("key1".to_string(), "b".to_string());
        groups.add("key2".to_string(), "c".to_string());
        let g = groups.groups();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].count, 2);
        assert_eq!(g[0].canonical_id, "a");
        assert!(groups.is_duplicate("b"));
        assert!(!groups.is_duplicate("a"));
        assert_eq!(groups.canonical_of("b").as_deref(), Some("a"));
        assert_eq!(groups.group_members("b").unwrap().len(), 2);
    }
}
