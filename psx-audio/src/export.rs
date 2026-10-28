// SPDX-License-Identifier: MIT
//! Writing rendered audio out.
//!
//! # Examples
//!
//! ```
//! let wav = psx_audio::export::wav_bytes(&[0i16; 8], 44_100, 2)?;
//! assert_eq!(&wav[..4], b"RIFF");
//! # Ok::<(), psx_audio::Error>(())
//! ```

use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::error::Result;

/// The Ogg Vorbis quality scale, `-q 0` (smallest) through `-q 10` (best).
///
/// # Examples
///
/// ```
/// assert!(psx_audio::export::OGG_QUALITY_RANGE.contains(&6.0));
/// ```
pub const OGG_QUALITY_RANGE: core::ops::RangeInclusive<f32> = 0.0..=10.0;

/// The Ogg Vorbis default: `-q 6`, about 192 kbps at 44.1 kHz stereo.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::export::DEFAULT_OGG_QUALITY, 6.0);
/// ```
pub const DEFAULT_OGG_QUALITY: f32 = 6.0;

/// A FLAC frame holds this many samples per channel.
const FLAC_BLOCK_SIZE: usize = 4096;

/// Samples per channel fed to the Vorbis encoder at a time.
#[cfg(feature = "ogg")]
const OGG_BLOCK_FRAMES: usize = 4096;

/// Vorbis-comment fields written beside the audio.
///
/// Empty strings and `None` fields are omitted. Loop positions are sample
/// frames per channel, as engines read `LOOPSTART` and `LOOPLENGTH`.
///
/// # Examples
///
/// ```
/// let tags = psx_audio::export::Tags {
///     title: "Test Tone",
///     loop_start: Some(44_100),
///     ..psx_audio::export::Tags::default()
/// };
/// assert_eq!(tags.title, "Test Tone");
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct Tags<'a> {
    /// The audio title.
    pub title: &'a str,
    /// The artist.
    pub artist: &'a str,
    /// The album.
    pub album: &'a str,
    /// The track number.
    pub track: Option<u32>,
    /// First frame of the loop.
    pub loop_start: Option<u64>,
    /// Frames in the loop.
    pub loop_length: Option<u64>,
}

impl Tags<'_> {
    /// The nonempty comment pairs, in a stable order.
    fn entries(&self) -> Vec<(&'static str, alloc::string::String)> {
        let mut entries = Vec::new();
        if !self.title.is_empty() {
            entries.push(("TITLE", self.title.into()));
        }
        if !self.artist.is_empty() {
            entries.push(("ARTIST", self.artist.into()));
        }
        if !self.album.is_empty() {
            entries.push(("ALBUM", self.album.into()));
        }
        if let Some(track) = self.track {
            entries.push(("TRACKNUMBER", track.to_string()));
        }
        if let Some(start) = self.loop_start {
            entries.push(("LOOPSTART", start.to_string()));
        }
        if let Some(length) = self.loop_length {
            entries.push(("LOOPLENGTH", length.to_string()));
        }
        entries
    }
}

/// A 16-bit PCM WAVE file for interleaved `samples`.
///
/// # Errors
///
/// Fails when the sample count does not hold whole frames or the rate is zero.
///
/// # Examples
///
/// ```
/// let wav = psx_audio::export::wav_bytes(&[1, -2], 44_100, 2)?;
/// assert_eq!(&wav[..4], b"RIFF");
/// # Ok::<(), psx_audio::Error>(())
/// ```
pub fn wav_bytes(samples: &[i16], sample_rate: u32, channels: u32) -> Result<Vec<u8>> {
    psx_formats::wav::pcm(samples, sample_rate, channels).map_err(Into::into)
}

/// Raw interleaved little-endian 16-bit samples.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::export::raw_s16le(&[1, -2]), [0x01, 0x00, 0xFE, 0xFF]);
/// ```
#[must_use]
pub fn raw_s16le(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// A lossless FLAC file for interleaved 16-bit `samples`, with `title` in the
/// Vorbis comment metadata.
///
/// # Errors
///
/// Fails when the sample count does not hold whole frames, or when the
/// encoder rejects the shape.
///
/// # Examples
///
/// ```
/// let flac = psx_audio::export::flac_bytes(&[0i16; 8], 44_100, 2, &psx_audio::export::Tags::default())?;
/// assert_eq!(&flac[..4], b"fLaC");
/// # Ok::<(), psx_audio::Error>(())
/// ```
pub fn flac_bytes(samples: &[i16], sample_rate: u32, channels: u32, tags: &Tags<'_>) -> Result<Vec<u8>> {
    use flacenc::component::{BitRepr, MetadataBlockData};
    use flacenc::error::Verify;

    let channels_usize = check_shape(samples, sample_rate, channels, "FLAC")?;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, error)| crate::Error::format(format!("FLAC encoder configuration: {error}")))?;
    let source = PcmSource { samples, channels: channels_usize, rate: sample_rate, read: 0, converted: Vec::new() };
    let mut stream = flacenc::encode_with_fixed_block_size(&config, source, FLAC_BLOCK_SIZE)
        .map_err(|error| crate::Error::format(format!("FLAC encoder: {error}")))?;
    let comment = flac_comment(tags);
    if !comment.is_empty() {
        stream.add_metadata_block(
            MetadataBlockData::new_unknown(4, &comment)
                .map_err(|error| crate::Error::format(format!("FLAC comments: {error}")))?,
        );
    }
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|error| crate::Error::format(format!("FLAC stream: {error}")))?;
    Ok(sink.as_slice().to_vec())
}

/// An Ogg Vorbis file for interleaved 16-bit `samples`, with `title` in the
/// Vorbis comment header.
///
/// `quality` is the Vorbis `-q` scale, [`OGG_QUALITY_RANGE`]; [`DEFAULT_OGG_QUALITY`]
/// is about 192 kbps at 44.1 kHz stereo.
///
/// # Errors
///
/// Fails when the quality is outside the range, the sample count does not hold
/// whole frames, or the encoder rejects the shape. Requires the `ogg` feature;
/// without it, the call returns an error naming the rebuild.
///
/// # Examples
///
/// ```
/// let ogg = psx_audio::export::ogg_bytes(
///     &[0i16; 8],
///     44_100,
///     2,
///     psx_audio::export::DEFAULT_OGG_QUALITY,
///     &psx_audio::export::Tags::default(),
/// )?;
/// assert_eq!(&ogg[..4], b"OggS");
/// # Ok::<(), psx_audio::Error>(())
/// ```
#[cfg(feature = "ogg")]
pub fn ogg_bytes(samples: &[i16], sample_rate: u32, channels: u32, quality: f32, tags: &Tags<'_>) -> Result<Vec<u8>> {
    use std::num::{NonZeroU8, NonZeroU32};

    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

    if !OGG_QUALITY_RANGE.contains(&quality) {
        return Err(crate::Error::invalid(format!(
            "Ogg quality {quality} is outside 0-10: pass --quality 0 (smallest) to 10 (best), for example --quality 6."
        )));
    }
    let channels_usize = check_shape(samples, sample_rate, channels, "Ogg Vorbis")?;
    let channel_count = u8::try_from(channels).ok().and_then(NonZeroU8::new).ok_or_else(|| {
        crate::Error::invalid(format!("Ogg Vorbis needs 1-255 channels, this render has {channels}."))
    })?;
    let rate =
        NonZeroU32::new(sample_rate).ok_or_else(|| crate::Error::invalid("Ogg Vorbis needs a nonzero sample rate."))?;
    let mut builder = VorbisEncoderBuilder::new(rate, channel_count, Vec::new())
        .map_err(|error| crate::Error::format(format!("Ogg Vorbis encoder: {error}")))?;
    // libvorbis takes the same quality in the 0-1 range the `-q` switch is scaled from.
    builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: quality / 10.0 });
    for (key, value) in tags.entries() {
        builder
            .comment_tag(key, value.as_str())
            .map_err(|error| crate::Error::format(format!("Ogg Vorbis comments: {error}")))?;
    }
    let mut encoder = builder.build().map_err(|error| crate::Error::format(format!("Ogg Vorbis encoder: {error}")))?;
    let mut planar: Vec<Vec<f32>> = (0..channels_usize).map(|_| Vec::with_capacity(OGG_BLOCK_FRAMES)).collect();
    for frame in samples.chunks_exact(channels_usize) {
        for (channel, &sample) in planar.iter_mut().zip(frame) {
            channel.push(f32::from(sample) / 32768.0);
        }
        if planar[0].len() == OGG_BLOCK_FRAMES {
            encoder
                .encode_audio_block(planar.iter().map(Vec::as_slice).collect::<Vec<_>>())
                .map_err(|error| crate::Error::format(format!("Ogg Vorbis encoder: {error}")))?;
            for channel in &mut planar {
                channel.clear();
            }
        }
    }
    if !planar[0].is_empty() {
        encoder
            .encode_audio_block(planar.iter().map(Vec::as_slice).collect::<Vec<_>>())
            .map_err(|error| crate::Error::format(format!("Ogg Vorbis encoder: {error}")))?;
    }
    encoder.finish().map_err(|error| crate::Error::format(format!("Ogg Vorbis encoder: {error}")))
}

/// Without the `ogg` feature the Ogg encoder is not linked; the call names the rebuild.
///
/// # Examples
///
/// ```
/// let error = psx_audio::export::ogg_bytes(
///     &[0i16; 2],
///     44_100,
///     2,
///     psx_audio::export::DEFAULT_OGG_QUALITY,
///     &psx_audio::export::Tags::default(),
/// )
/// .unwrap_err();
/// assert!(error.message().contains("ogg feature"));
/// ```
#[cfg(not(feature = "ogg"))]
pub fn ogg_bytes(
    _samples: &[i16],
    _sample_rate: u32,
    _channels: u32,
    _quality: f32,
    _tags: &Tags<'_>,
) -> Result<Vec<u8>> {
    Err(crate::Error::Unsupported("Ogg export is not built in: rebuild with the ogg feature".into()))
}

/// Validates the shape both encoders need and returns the channel count.
fn check_shape(samples: &[i16], sample_rate: u32, channels: u32, codec: &str) -> Result<usize> {
    if channels == 0 || channels > 8 {
        return Err(crate::Error::invalid(format!("{codec} needs 1-8 channels, this render has {channels}.")));
    }
    if sample_rate == 0 {
        return Err(crate::Error::invalid(format!("{codec} needs a nonzero sample rate.")));
    }
    let channels = channels as usize;
    if !samples.len().is_multiple_of(channels) {
        return Err(crate::Error::invalid(format!("{codec} needs whole frames, not {} samples.", samples.len())));
    }
    Ok(channels)
}

/// The FLAC VORBIS_COMMENT metadata block, empty when there is no tag.
fn flac_comment(tags: &Tags<'_>) -> Vec<u8> {
    let entries = tags.entries();
    if entries.is_empty() {
        return Vec::new();
    }
    let vendor = concat!("psx-audio ", env!("CARGO_PKG_VERSION"));
    let mut comment = Vec::new();
    comment.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    comment.extend_from_slice(vendor.as_bytes());
    comment.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (key, value) in entries {
        let tag = format!("{key}={value}");
        comment.extend_from_slice(&(tag.len() as u32).to_le_bytes());
        comment.extend_from_slice(tag.as_bytes());
    }
    comment
}

/// Interleaved 16-bit PCM as the `i32` frames `flacenc` reads, converted one
/// block at a time so a long export holds no second copy of the track.
struct PcmSource<'a> {
    samples: &'a [i16],
    channels: usize,
    rate: u32,
    read: usize,
    converted: Vec<i32>,
}

impl flacenc::source::Source for PcmSource<'_> {
    fn channels(&self) -> usize {
        self.channels
    }

    fn bits_per_sample(&self) -> usize {
        16
    }

    fn sample_rate(&self) -> usize {
        self.rate as usize
    }

    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> core::result::Result<usize, flacenc::error::SourceError> {
        let begin = self.read * self.channels;
        let end = (begin + block_size * self.channels).min(self.samples.len());
        self.converted.clear();
        self.converted.extend(self.samples[begin..end].iter().map(|&sample| i32::from(sample)));
        dest.fill_interleaved(&self.converted)?;
        let read = (end - begin) / self.channels;
        self.read += read;
        Ok(read)
    }

    fn len_hint(&self) -> Option<usize> {
        Some(self.samples.len() / self.channels)
    }
}
