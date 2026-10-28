//! RIFF/WAVE: the shape of a 16-bit PCM file and the one writer for it.
//!
//! The writer takes interleaved 16-bit samples and returns the file bytes. The
//! header is the canonical 44-byte PCM form, the one every reader takes.
//!
//! # Examples
//!
//! ```
//! use psx_formats::wav;
//!
//! let bytes = wav::pcm(&[0i16; 2], 44_100, 1).unwrap();
//! assert_eq!(wav::parse(&bytes).unwrap().data_bytes, 4);
//! ```

use alloc::vec::Vec;

use crate::bytes::{u16_le, u32_be, u32_le};
use crate::error::{Error, Result};

const RIFF: u32 = 0x5249_4646;
const WAVE: u32 = 0x5741_5645;
const FMT: u32 = 0x666D_7420;
const DATA: u32 = 0x6461_7461;
const RIFF_HEAD: usize = 12;
const CHUNK_HEAD: usize = 8;
const FMT_BYTES: usize = 16;
const PCM_TAG: u16 = 1;
const HEADER_BYTES: usize = 44;
const BITS_PER_SAMPLE: u16 = 16;
const BYTES_PER_SAMPLE: u32 = 2;

/// Shape of a PCM WAVE file.
///
/// # Examples
///
/// ```
/// let bytes = psx_formats::wav::pcm(&[0i16; 4], 44_100, 1).unwrap();
/// let info = psx_formats::wav::parse(&bytes).unwrap();
/// assert_eq!((info.channels, info.rate, info.bits), (1, 44_100, 16));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    /// Channel count.
    pub channels: u32,
    /// Sample rate (Hz).
    pub rate: u32,
    /// Bits per sample.
    pub bits: u32,
    /// Byte offset of the sample data.
    pub data_offset: u64,
    /// Declared size of the sample data.
    pub data_bytes: u64,
}

impl Info {
    /// Frames the declared data holds.
    ///
    /// # Examples
    ///
    /// ```
    /// let bytes = psx_formats::wav::pcm(&[0i16; 6], 44_100, 2).unwrap();
    /// assert_eq!(psx_formats::wav::parse(&bytes).unwrap().frames(), 3);
    /// ```
    #[must_use]
    pub fn frames(&self) -> u64 {
        let frame = u64::from(self.channels) * u64::from(self.bits / 8);
        self.data_bytes.checked_div(frame).unwrap_or(0)
    }
}

/// Read the shape from the first bytes of a file (a few KiB are enough).
///
/// # Examples
///
/// ```
/// let bytes = psx_formats::wav::pcm(&[0i16; 2], 22_050, 1).unwrap();
/// let info = psx_formats::wav::parse(&bytes).unwrap();
/// assert_eq!(info.rate, 22_050);
/// assert_eq!(info.data_bytes, 4);
/// ```
pub fn parse(head: &[u8]) -> Result<Info> {
    if head.len() < RIFF_HEAD || u32_be(head, 0) != RIFF || u32_be(head, 8) != WAVE {
        return Err(Error::WaveFormat("not a RIFF/WAVE file"));
    }
    let (mut channels, mut rate, mut bits) = (0u32, 0u32, 0u32);
    let mut at = RIFF_HEAD;
    while at + CHUNK_HEAD <= head.len() {
        let id = u32_be(head, at);
        let size = u32_le(head, at + 4) as usize;
        if id == DATA {
            if rate == 0 || channels == 0 {
                return Err(Error::WaveFormat("WAVE data chunk precedes its format"));
            }
            return Ok(Info { channels, rate, bits, data_offset: (at + CHUNK_HEAD) as u64, data_bytes: size as u64 });
        }
        if size > head.len() - at - CHUNK_HEAD {
            return Err(Error::WaveFormat("WAVE chunk runs past the head of the file"));
        }
        if id == FMT {
            if size < FMT_BYTES {
                return Err(Error::WaveFormat("WAVE fmt chunk is truncated"));
            }
            if u16_le(head, at + 8) != PCM_TAG {
                return Err(Error::WaveUnsupported("WAVE is not uncompressed PCM"));
            }
            channels = u32::from(u16_le(head, at + 10));
            rate = u32_le(head, at + 12);
            bits = u32::from(u16_le(head, at + 22));
        }
        at += CHUNK_HEAD + size + (size & 1);
    }
    Err(Error::WaveFormat("WAVE file has no data chunk"))
}

/// A 16-bit PCM WAVE file for interleaved `samples`.
///
/// # Examples
///
/// ```
/// use psx_formats::wav;
///
/// let bytes = wav::pcm(&[1, -1, 1, -1], 44_100, 2).unwrap();
/// assert_eq!(bytes.len(), 44 + 8, "a 44-byte header and four 16-bit samples");
/// assert_eq!(&bytes[..4], b"RIFF");
/// ```
pub fn pcm(samples: &[i16], sample_rate: u32, channels: u32) -> Result<Vec<u8>> {
    if channels == 0 {
        return Err(Error::WaveShape("a WAVE file needs at least one channel"));
    }
    if !samples.len().is_multiple_of(channels as usize) {
        return Err(Error::WaveShape("the samples do not hold whole frames"));
    }
    let channels = u16::try_from(channels).map_err(|_| Error::WaveShape("too many channels"))?;
    let data_bytes = u32::try_from(samples.len() * BYTES_PER_SAMPLE as usize)
        .map_err(|_| Error::WaveShape("audio is too long for a WAVE file"))?;
    let mut out = Vec::with_capacity(HEADER_BYTES + samples.len() * BYTES_PER_SAMPLE as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&PCM_TAG.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * u32::from(channels) * BYTES_PER_SAMPLE).to_le_bytes());
    out.extend_from_slice(&(channels * BYTES_PER_SAMPLE as u16).to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(out)
}
