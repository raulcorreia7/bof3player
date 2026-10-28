//! CD-ROM XA ADPCM streams (psx-spx, "CDROM XA Subheader, File, Channel,
//! Interleave" and "CDROM XA Audio ADPCM Compression").
//!
//! A stream is a run of 2336-byte sectors (8-byte subheader, 2324 data bytes:
//! eighteen 128-byte sound groups and 20 bytes of padding). Channels are
//! interleaved by the subheader's channel byte.
//!
//! # Examples
//!
//! ```
//! let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
//! sector[2] = 0x04;
//! assert_eq!(psx_formats::xa::scan(&sector).len(), 1);
//! ```

use alloc::vec::Vec;

use crate::adpcm::{self, BLOCK_SAMPLES, Rounding, State};
use crate::error::{Error, Result};

/// Bytes of one sector as stored in an extracted stream (subheader + data + EDC).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::xa::SECTOR_BYTES, 2336);
/// ```
pub const SECTOR_BYTES: usize = 2336;
/// Normal sample rate (Hz).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::xa::RATE, 37_800);
/// ```
pub const RATE: u32 = 37_800;
/// Low sample rate selected by coding bit 2 (Hz).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::xa::rate(0x04), psx_formats::xa::RATE_LOW);
/// ```
pub const RATE_LOW: u32 = 18_900;

const SUBHEADER_BYTES: usize = 8;
const GROUPS_PER_SECTOR: usize = 18;
const GROUP_BYTES: usize = 128;
const BLOCKS_PER_GROUP: usize = 4;
/// Offset of the block headers in a sound group.
const HEADER_AT: usize = 4;
/// Offset of the data words in a sound group.
const DATA_AT: usize = 16;
/// Submode bit 2: audio sector.
const SUBMODE_AUDIO: u8 = 0x04;
/// Submode bit 7: end of file.
const SUBMODE_EOF: u8 = 0x80;
/// Coding bit 0: stereo.
const CODING_STEREO: u8 = 0x01;
/// Coding bit 2: 18.9 kHz.
const CODING_LOW_RATE: u8 = 0x04;
/// XA filters use two bits (psx-spx: "only four filters").
const FILTER_MASK: u8 = 0x03;

const MONO_FRAMES_PER_GROUP: usize = BLOCKS_PER_GROUP * 2 * BLOCK_SAMPLES;
const STEREO_FRAMES_PER_GROUP: usize = BLOCKS_PER_GROUP * BLOCK_SAMPLES;

/// Sample rate for a coding byte.
///
/// # Examples
///
/// ```
/// use psx_formats::xa::{RATE, RATE_LOW, rate};
///
/// assert_eq!(rate(0x00), RATE);
/// assert_eq!(rate(0x04), RATE_LOW);
/// ```
#[must_use]
pub fn rate(coding: u8) -> u32 {
    if coding & CODING_LOW_RATE != 0 { RATE_LOW } else { RATE }
}

/// Output channels (1 or 2) for a coding byte.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::xa::channels(0x00), 1);
/// assert_eq!(psx_formats::xa::channels(0x01), 2);
/// ```
#[must_use]
pub fn channels(coding: u8) -> u32 {
    if coding & CODING_STEREO != 0 { 2 } else { 1 }
}

/// Frames in one sector.
///
/// # Examples
///
/// ```
/// use psx_formats::xa::frames_per_sector;
///
/// assert_eq!(frames_per_sector(0x00), 4032);
/// assert_eq!(frames_per_sector(0x01), 2016);
/// ```
#[must_use]
pub fn frames_per_sector(coding: u8) -> u32 {
    let per_group = if coding & CODING_STEREO != 0 { STEREO_FRAMES_PER_GROUP } else { MONO_FRAMES_PER_GROUP };
    (GROUPS_PER_SECTOR * per_group) as u32
}

/// Duration of `sectors` sectors of one channel, in seconds.
///
/// # Examples
///
/// ```
/// use psx_formats::xa::{duration, frames_per_sector, rate};
///
/// let seconds = duration(18, 0x00);
/// assert!((seconds - f64::from(18 * frames_per_sector(0x00)) / f64::from(rate(0x00))).abs() < 1e-12);
/// ```
#[must_use]
pub fn duration(sectors: u32, coding: u8) -> f64 {
    f64::from(sectors) * f64::from(frames_per_sector(coding)) / f64::from(rate(coding))
}

/// Whether `sector` (2336 bytes) is an audio sector.
///
/// # Examples
///
/// ```
/// let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
/// sector[2] = 0x04;
/// assert!(psx_formats::xa::is_audio(&sector));
/// ```
#[must_use]
pub fn is_audio(sector: &[u8]) -> bool {
    sector.len() >= SUBHEADER_BYTES && sector[2] & SUBMODE_AUDIO != 0
}

/// Whether `sector` carries the end-of-file bit.
///
/// # Examples
///
/// ```
/// let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
/// sector[2] = 0x80;
/// assert!(psx_formats::xa::is_eof(&sector));
/// ```
#[must_use]
pub fn is_eof(sector: &[u8]) -> bool {
    sector.len() >= SUBHEADER_BYTES && sector[2] & SUBMODE_EOF != 0
}

/// A run of one channel's sectors closed by an EOF marker.
///
/// # Examples
///
/// ```
/// let span = psx_formats::xa::Span { first: 2, eof: 5, sectors: 4 };
/// assert_eq!(span.eof - span.first, 3);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    /// Stream sector index of the first audio sector.
    pub first: u32,
    /// Stream sector index of the EOF sector.
    pub eof: u32,
    /// Audio sectors of this channel in the span.
    pub sectors: u32,
}

/// What a scan found for one (file, channel) pair.
///
/// # Examples
///
/// ```
/// let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
/// sector[2] = 0x04;
/// let channels = psx_formats::xa::scan(&sector);
/// assert_eq!((channels[0].file, channels[0].sectors), (0, 1));
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    /// Subheader file number.
    pub file: u8,
    /// Subheader channel number.
    pub channel: u8,
    /// Coding byte of the first audio sector.
    pub coding: u8,
    /// Total audio sectors.
    pub sectors: u32,
    /// Stream sector index of the first audio sector.
    pub first: u32,
    /// Duration of the whole channel, seconds.
    pub seconds: f64,
    /// EOF-closed spans, in order.
    pub spans: Vec<Span>,
    /// First sector of a trailing run the stream never closed.
    pub open_first: u32,
    /// Sectors in that run.
    pub open_sectors: u32,
}

/// Incremental scanner: feed sectors in stream order.
///
/// # Examples
///
/// ```
/// use psx_formats::xa::{SECTOR_BYTES, Scanner};
///
/// let mut sector = vec![0u8; SECTOR_BYTES];
/// sector[2] = 0x04;
/// let mut scanner = Scanner::new();
/// scanner.push(&sector);
/// assert_eq!(scanner.finish().len(), 1);
/// ```
#[derive(Debug, Default)]
pub struct Scanner {
    channels: Vec<(Channel, usize)>,
    index: u32,
}

impl Scanner {
    /// A scanner at stream sector 0.
    ///
    /// # Examples
    ///
    /// ```
    /// let scanner = psx_formats::xa::Scanner::new();
    /// assert!(scanner.finish().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Account for the next sector of the stream.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
    /// sector[2] = 0x04;
    /// let mut scanner = psx_formats::xa::Scanner::new();
    /// scanner.push(&sector);
    /// assert_eq!(scanner.finish()[0].sectors, 1);
    /// ```
    pub fn push(&mut self, sector: &[u8]) {
        let at = self.index;
        self.index += 1;
        if !is_audio(sector) {
            return;
        }
        let slot = match self.channels.iter().position(|(c, _)| c.file == sector[0] && c.channel == sector[1]) {
            Some(slot) => slot,
            None => {
                self.channels.push((
                    Channel { file: sector[0], channel: sector[1], coding: sector[3], first: at, ..Channel::default() },
                    0,
                ));
                self.channels.len() - 1
            }
        };
        let (channel, closed) = &mut self.channels[slot];
        if channel.spans.len() == *closed {
            channel.spans.push(Span::default());
        }
        channel.sectors += 1;
        let span = channel.spans.last_mut().expect("span just ensured");
        if span.sectors == 0 {
            span.first = at;
        }
        span.sectors += 1;
        if is_eof(sector) {
            span.eof = at;
            *closed += 1;
        }
    }

    /// Finish: every EOF closed one span, a span left over is the open run.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
    /// sector[2] = 0x04;
    /// let mut scanner = psx_formats::xa::Scanner::new();
    /// scanner.push(&sector);
    /// assert_eq!(scanner.finish()[0].open_sectors, 1);
    /// ```
    #[must_use]
    pub fn finish(self) -> Vec<Channel> {
        self.channels
            .into_iter()
            .map(|(mut c, closed)| {
                if c.spans.len() > closed {
                    let open = c.spans[c.spans.len() - 1];
                    c.open_first = open.first;
                    c.open_sectors = open.sectors;
                }
                c.spans.truncate(closed);
                c.seconds = duration(c.sectors, c.coding);
                c
            })
            .collect()
    }
}

/// Scan a whole in-memory stream.
///
/// # Examples
///
/// ```
/// let mut sector = vec![0u8; psx_formats::xa::SECTOR_BYTES];
/// sector[2] = 0x04;
/// sector[3] = 0x01;
/// let channels = psx_formats::xa::scan(&sector);
/// assert_eq!(channels[0].seconds, psx_formats::xa::duration(1, 0x01));
/// ```
#[must_use]
pub fn scan(stream: &[u8]) -> Vec<Channel> {
    let mut scanner = Scanner::new();
    for sector in stream.as_chunks::<SECTOR_BYTES>().0 {
        scanner.push(sector);
    }
    scanner.finish()
}

/// Decodes the sectors of one channel, keeping the predictor chains running
/// across sectors.
///
/// # Examples
///
/// ```
/// use psx_formats::xa::{ChannelDecoder, SECTOR_BYTES, frames_per_sector};
///
/// let mut sector = vec![0u8; SECTOR_BYTES];
/// sector[2] = 0x04;
/// let mut decoder = ChannelDecoder::new(0);
/// let mut pcm = Vec::new();
/// decoder.decode(&sector, &mut pcm).unwrap();
/// assert_eq!(pcm.len(), frames_per_sector(0) as usize);
/// ```
#[derive(Debug, Clone)]
pub struct ChannelDecoder {
    channel: u8,
    left: State,
    right: State,
    shape: Option<(u32, u32)>,
}

impl ChannelDecoder {
    /// A decoder for `channel`.
    ///
    /// # Examples
    ///
    /// ```
    /// let decoder = psx_formats::xa::ChannelDecoder::new(3);
    /// assert_eq!(decoder.shape(), None);
    /// ```
    #[must_use]
    pub fn new(channel: u8) -> Self {
        Self { channel, left: State::default(), right: State::default(), shape: None }
    }

    /// Whether `sector` is an audio sector of this decoder's channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::xa::{ChannelDecoder, SECTOR_BYTES};
    ///
    /// let mut sector = vec![0u8; SECTOR_BYTES];
    /// sector[2] = 0x04;
    /// assert!(ChannelDecoder::new(0).accepts(&sector));
    /// assert!(!ChannelDecoder::new(1).accepts(&sector));
    /// ```
    #[must_use]
    pub fn accepts(&self, sector: &[u8]) -> bool {
        sector.len() == SECTOR_BYTES && is_audio(sector) && sector[1] == self.channel
    }

    /// `(channels, rate)` once a sector has been decoded.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::xa::{ChannelDecoder, SECTOR_BYTES};
    ///
    /// let mut sector = vec![0u8; SECTOR_BYTES];
    /// sector[2] = 0x04;
    /// let mut decoder = ChannelDecoder::new(0);
    /// decoder.decode(&sector, &mut Vec::new()).unwrap();
    /// assert_eq!(decoder.shape(), Some((1, 37_800)));
    /// ```
    #[must_use]
    pub fn shape(&self) -> Option<(u32, u32)> {
        self.shape
    }

    /// Decode one accepted sector, appending interleaved samples to `out`.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_formats::xa::{ChannelDecoder, SECTOR_BYTES};
    ///
    /// let mut sector = vec![0u8; SECTOR_BYTES];
    /// sector[2] = 0x04;
    /// let mut pcm = Vec::new();
    /// ChannelDecoder::new(0).decode(&sector, &mut pcm).unwrap();
    /// assert!(!pcm.is_empty());
    /// ```
    pub fn decode(&mut self, sector: &[u8], out: &mut Vec<i16>) -> Result<()> {
        if sector.len() != SECTOR_BYTES {
            return Err(Error::XaSectorSize(sector.len()));
        }
        if !self.accepts(sector) {
            return Ok(());
        }
        let coding = sector[3];
        let channels = channels(coding);
        self.shape.get_or_insert((channels, rate(coding)));
        let payload = &sector[SUBHEADER_BYTES..];
        for group in 0..GROUPS_PER_SECTOR {
            let data = &payload[group * GROUP_BYTES..(group + 1) * GROUP_BYTES];
            if channels == 1 {
                let mut mono = [0i16; MONO_FRAMES_PER_GROUP];
                for block in 0..BLOCKS_PER_GROUP {
                    for nibble in 0..2 {
                        let start = (block * 2 + nibble) * BLOCK_SAMPLES;
                        decode_run(data, block, nibble, &mut mono[start..start + BLOCK_SAMPLES], &mut self.left);
                    }
                }
                out.extend_from_slice(&mono);
            } else {
                let mut lefts = [0i16; STEREO_FRAMES_PER_GROUP];
                let mut rights = [0i16; STEREO_FRAMES_PER_GROUP];
                for block in 0..BLOCKS_PER_GROUP {
                    let range = block * BLOCK_SAMPLES..(block + 1) * BLOCK_SAMPLES;
                    decode_run(data, block, 0, &mut lefts[range.clone()], &mut self.left);
                    decode_run(data, block, 1, &mut rights[range], &mut self.right);
                }
                for (l, r) in lefts.iter().zip(&rights) {
                    out.push(*l);
                    out.push(*r);
                }
            }
        }
        Ok(())
    }
}

/// One 28-sample run: block `block` (0..4) read with `nibble` (0 = low half).
///
/// psx-spx `decode_28_nibbles`: the header byte is at `4 + block*2 + nibble`,
/// sample `j` at `16 + block + j*4`.
fn decode_run(group: &[u8], block: usize, nibble: usize, out: &mut [i16], state: &mut State) {
    let header = group[HEADER_AT + block * 2 + nibble];
    let shift = header & 0x0F;
    let filter = usize::from((header >> 4) & FILTER_MASK);
    for (j, sample) in out.iter_mut().enumerate() {
        let byte = group[DATA_AT + block + j * BLOCKS_PER_GROUP];
        let value = (byte >> (nibble * 4)) & 0x0F;
        *sample = adpcm::step(value, shift, filter, state, Rounding::RoundSum);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::sector;

    #[test]
    fn scan_groups_channels_and_closes_spans() {
        let mut stream = Vec::new();
        for i in 0..6u8 {
            let eof = if i == 2 { SUBMODE_EOF } else { 0 };
            stream.extend(sector(1, i % 2, SUBMODE_AUDIO | eof, 1));
        }
        stream.extend(sector(1, 0, 0, 0)); // non-audio sector is skipped
        let channels = scan(&stream);
        assert_eq!(channels.len(), 2);
        let ch0 = &channels[0];
        assert_eq!((ch0.channel, ch0.sectors, ch0.spans.len()), (0, 3, 1));
        assert_eq!(ch0.spans[0], Span { first: 0, eof: 2, sectors: 2 });
        assert_eq!((ch0.open_first, ch0.open_sectors), (4, 1));
        assert!((ch0.seconds - duration(3, 1)).abs() < 1e-12);
    }

    #[test]
    fn shapes_follow_coding_byte() {
        assert_eq!((rate(0), channels(0)), (37_800, 1));
        assert_eq!((rate(5), channels(5)), (18_900, 2));
        assert_eq!(frames_per_sector(1), 18 * 112);
        assert_eq!(frames_per_sector(0), 18 * 224);
    }

    /// Golden vector: filter 0, shift 12, one nonzero nibble per run.
    #[test]
    fn stereo_group_layout_and_golden_samples() {
        let mut s = sector(1, 0, SUBMODE_AUDIO, 1);
        let group = &mut s[SUBHEADER_BYTES..SUBHEADER_BYTES + GROUP_BYTES];
        // Headers: shift 12 for block 0 left and right, filter 0.
        group[HEADER_AT] = 0x0C;
        group[HEADER_AT + 1] = 0x0C;
        // First data word, block 0: left nibble 3, right nibble 5.
        group[DATA_AT] = 0x53;
        let mut out = Vec::new();
        let mut decoder = ChannelDecoder::new(0);
        decoder.decode(&s, &mut out).unwrap();
        assert_eq!(out.len(), 18 * 112 * 2);
        assert_eq!(&out[..4], &[3, 5, 0, 0]);
        assert_eq!(decoder.shape(), Some((2, 37_800)));
    }

    #[test]
    fn decoder_ignores_other_channels() {
        let mut out = Vec::new();
        let mut decoder = ChannelDecoder::new(3);
        decoder.decode(&sector(1, 0, SUBMODE_AUDIO, 1), &mut out).unwrap();
        assert!(out.is_empty());
        assert!(decoder.decode(&[0u8; 10], &mut out).is_err());
    }
}
