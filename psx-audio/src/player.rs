// SPDX-License-Identifier: MIT
//! Pulling PCM out of a PlayStation stream.
//!
//! [`Player`] owns an [`Engine`] and adds gain, stop/play and the float view. The crate ships
//! XA-stream and CD-DA engines ([`Player::xa`], [`Player::cdda`]); any other decoder implements
//! [`Engine`] and is played through [`Player::new`].

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

use psx_formats::wav;
use psx_formats::xa::{self, ChannelDecoder};

use crate::error::{Error, Result};
use crate::source::Blob;

/// Frames a seek renders per pass while catching up.
const SEEK_CHUNK_FRAMES: usize = 2048;
/// Frames decoded per read from a CD-DA track.
const CDDA_WINDOW_FRAMES: usize = 8192;
/// Sectors read per window from an XA stream.
const XA_WINDOW_SECTORS: usize = 32;
/// CD-DA is fixed by the format: 44.1 kHz, two channels, 16-bit.
const CDDA_RATE: u32 = 44_100;
const CDDA_CHANNELS: u32 = 2;
const CDDA_BITS: u32 = 16;
/// Enough of a WAV header for a chunk list with a LIST chunk before the data.
const WAV_HEAD_BYTES: usize = 4096;
/// Gain limits.
const GAIN_MAX: f32 = 4.0;
/// Fixed-point unity of the gain stage.
const GAIN_ONE: i32 = 4096;
const GAIN_BITS: u32 = 12;

/// Loop repetition request.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::Loop::default(), psx_audio::Loop::Off);
/// assert_ne!(psx_audio::Loop::Forever, psx_audio::Loop::Times(1));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Loop {
    /// Play straight through once.
    #[default]
    Off,
    /// Play the loop window this many extra times, then run on to the end.
    Times(u32),
    /// Never end.
    Forever,
}

/// A source of interleaved PCM that [`Player`] drives.
///
/// Implement it to play your own decoder through the player's gain, stop/play and float views,
/// or use the built-in [`Player::xa`] and [`Player::cdda`] constructors.
///
/// # Examples
///
/// ```
/// use psx_audio::{Engine, Player};
///
/// struct Silence;
///
/// impl Engine for Silence {
///     fn render(&mut self, out: &mut [i16]) -> psx_audio::Result<usize> {
///         out.fill(0);
///         Ok(out.len() / 2)
///     }
///     fn seek(&mut self, _seconds: f64) -> psx_audio::Result<()> { Ok(()) }
///     fn rate(&self) -> u32 { 44_100 }
///     fn channels(&self) -> u32 { 2 }
///     fn duration(&self) -> f64 { 1.0 }
///     fn position(&self) -> f64 { 0.0 }
/// }
///
/// assert_eq!(Player::new(Silence).channels(), 2);
/// ```
pub trait Engine: Send {
    /// Render up to `out.len()` samples and return the frames written, 0 at the end.
    ///
    /// `out` holds whole interleaved frames: a multiple of [`Engine::channels`].
    fn render(&mut self, out: &mut [i16]) -> Result<usize>;

    /// Move to `seconds`, clamped to the stream.
    fn seek(&mut self, seconds: f64) -> Result<()>;

    /// Native sample rate in Hz. The player never resamples.
    fn rate(&self) -> u32;

    /// Interleaved channels per frame.
    fn channels(&self) -> u32;

    /// Length of one pass in seconds.
    fn duration(&self) -> f64;

    /// Current position in seconds.
    fn position(&self) -> f64;

    /// The loop window `(start, end)` in seconds, when the stream carries one.
    fn loop_window(&self) -> Option<(f64, f64)> {
        None
    }

    /// Repeat the loop window; engines without one ignore it.
    fn set_loop(&mut self, _mode: Loop) {}
}

/// Which sectors of an XA stream to play.
///
/// # Examples
///
/// ```
/// let stream = psx_audio::XaStream { channel: 0, first_sector: 0, sector_count: 16, duration: 1.0 };
/// assert_eq!(stream.sector_count, 16);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct XaStream {
    /// Subheader channel of the stream.
    pub channel: u8,
    /// First stream sector of the window.
    pub first_sector: u32,
    /// Sectors of the channel to play; 0 plays to the next EOF marker.
    pub sector_count: u32,
    /// Length of the window in seconds, when known.
    pub duration: f64,
}

struct XaEngine<'a> {
    blob: Arc<dyn Blob + 'a>,
    piece: XaStream,
    decoder: ChannelDecoder,
    window: Vec<u8>,
    window_sectors: usize,
    sector_index: usize,
    next_read_sector: u32,
    pending: Vec<i16>,
    pending_cursor: usize,
    decoded_sectors: u32,
    frames: i64,
    ended: bool,
    channels: u32,
    rate: u32,
    scratch: Vec<i16>,
}

impl<'a> XaEngine<'a> {
    fn new(blob: Arc<dyn Blob + 'a>, piece: XaStream) -> Result<Self> {
        let mut engine = Self {
            blob,
            piece,
            decoder: ChannelDecoder::new(piece.channel),
            window: alloc::vec![0u8; XA_WINDOW_SECTORS * xa::SECTOR_BYTES],
            window_sectors: 0,
            sector_index: 0,
            next_read_sector: piece.first_sector,
            pending: Vec::new(),
            pending_cursor: 0,
            decoded_sectors: 0,
            frames: 0,
            ended: false,
            channels: 0,
            rate: 0,
            scratch: Vec::new(),
        };
        // Decode up to the channel's first sector so the shape is known before the device opens.
        while !engine.ended && engine.channels == 0 {
            engine.decode_next_sector()?;
        }
        if engine.channels == 0 {
            return Err(Error::format(format!("XA stream has no audio sectors for channel {}", piece.channel)));
        }
        Ok(engine)
    }

    fn rewind(&mut self) {
        self.decoder = ChannelDecoder::new(self.piece.channel);
        self.next_read_sector = self.piece.first_sector;
        self.window_sectors = 0;
        self.sector_index = 0;
        self.pending.clear();
        self.pending_cursor = 0;
        self.decoded_sectors = 0;
        self.frames = 0;
        self.ended = false;
    }

    fn load_window(&mut self) -> Result<bool> {
        let offset = u64::from(self.next_read_sector) * xa::SECTOR_BYTES as u64;
        let size = self.blob.len();
        if offset >= size {
            return Ok(false);
        }
        let want = self.window.len().min((size - offset) as usize);
        let got = self.blob.read_at(offset, &mut self.window[..want])?;
        self.window_sectors = got / xa::SECTOR_BYTES;
        self.sector_index = 0;
        self.next_read_sector += self.window_sectors as u32;
        Ok(self.window_sectors != 0)
    }

    fn decode_next_sector(&mut self) -> Result<()> {
        loop {
            if self.sector_index >= self.window_sectors && !self.load_window()? {
                self.ended = true;
                return Ok(());
            }
            let at = self.sector_index * xa::SECTOR_BYTES;
            self.sector_index += 1;
            let sector = &self.window[at..at + xa::SECTOR_BYTES];
            if !self.decoder.accepts(sector) {
                continue;
            }
            let eof = xa::is_eof(sector);
            self.decoder.decode(sector, &mut self.pending)?;
            if let Some((channels, rate)) = self.decoder.shape() {
                self.channels = channels;
                self.rate = rate;
            }
            self.decoded_sectors += 1;
            // The EOF bit closes this channel's span: deliver the samples, then stop.
            if (self.piece.sector_count == 0 && eof)
                || (self.piece.sector_count != 0 && self.decoded_sectors >= self.piece.sector_count)
            {
                self.ended = true;
            }
            return Ok(());
        }
    }

    fn pump(&mut self, out: &mut [i16]) -> Result<usize> {
        let channels = self.channels as usize;
        let requested = out.len() / channels;
        let mut written = 0;
        while written < requested {
            if self.pending_cursor < self.pending.len() {
                let take = (self.pending.len() - self.pending_cursor).min((requested - written) * channels);
                out[written * channels..written * channels + take]
                    .copy_from_slice(&self.pending[self.pending_cursor..self.pending_cursor + take]);
                self.pending_cursor += take;
                written += take / channels;
                continue;
            }
            if self.ended {
                break;
            }
            self.pending.clear();
            self.pending_cursor = 0;
            self.decode_next_sector()?;
        }
        self.frames += written as i64;
        Ok(written)
    }
}

impl Engine for XaEngine<'_> {
    fn render(&mut self, out: &mut [i16]) -> Result<usize> {
        self.pump(out)
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let seconds = if seconds > 0.0 { seconds.min(self.duration()) } else { 0.0 };
        let target = (seconds * f64::from(self.rate)) as i64;
        if target < self.frames {
            self.rewind();
        }
        let channels = self.channels as usize;
        let mut scratch = core::mem::take(&mut self.scratch);
        scratch.resize(SEEK_CHUNK_FRAMES * channels, 0);
        while self.frames < target {
            let n = ((target - self.frames) as usize).min(SEEK_CHUNK_FRAMES);
            if self.pump(&mut scratch[..n * channels])? == 0 {
                break;
            }
        }
        self.scratch = scratch;
        Ok(())
    }

    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u32 {
        self.channels
    }
    fn duration(&self) -> f64 {
        self.piece.duration.max(0.0)
    }
    fn position(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate)
    }
}

struct CddaEngine<'a> {
    blob: Arc<dyn Blob + 'a>,
    channels: u32,
    rate: u32,
    data_offset: u64,
    total_frames: u64,
    frames: u64,
    window: Vec<u8>,
}

impl<'a> CddaEngine<'a> {
    /// `is_raw` selects raw 2352-byte sectors (`.RAW`) instead of a WAVE file.
    fn new(blob: Arc<dyn Blob + 'a>, is_raw: bool) -> Result<Self> {
        let (channels, rate, data_offset, data_bytes) = if is_raw {
            (CDDA_CHANNELS, CDDA_RATE, 0, blob.len())
        } else {
            let mut head = alloc::vec![0u8; WAV_HEAD_BYTES.min(blob.len() as usize)];
            let got = blob.read_at(0, &mut head)?;
            head.truncate(got);
            let info = wav::parse(&head)?;
            if info.bits != CDDA_BITS {
                return Err(Error::Unsupported(format!("CD-DA needs 16-bit PCM, this file has {} bits", info.bits)));
            }
            let available = blob.len().saturating_sub(info.data_offset);
            (info.channels, info.rate, info.data_offset, info.data_bytes.min(available))
        };
        let frame_bytes = u64::from(channels) * 2;
        if frame_bytes == 0 || data_bytes < frame_bytes {
            return Err(Error::format("CD-DA track holds no audio"));
        }
        Ok(Self {
            blob,
            channels,
            rate,
            data_offset,
            total_frames: data_bytes / frame_bytes,
            frames: 0,
            window: alloc::vec![0u8; CDDA_WINDOW_FRAMES * frame_bytes as usize],
        })
    }
}

impl Engine for CddaEngine<'_> {
    fn render(&mut self, out: &mut [i16]) -> Result<usize> {
        let channels = self.channels as usize;
        let frame_bytes = channels * 2;
        let frames = (out.len() / channels).min((self.total_frames - self.frames) as usize);
        let mut written = 0;
        while written < frames {
            let take = (frames - written).min(CDDA_WINDOW_FRAMES);
            let bytes = &mut self.window[..take * frame_bytes];
            let offset = self.data_offset + self.frames * frame_bytes as u64;
            let got = self.blob.read_at(offset, bytes)? / frame_bytes;
            if got == 0 {
                break;
            }
            for (sample, pair) in
                out[written * channels..(written + got) * channels].iter_mut().zip(bytes.as_chunks::<2>().0)
            {
                *sample = i16::from_le_bytes(*pair);
            }
            written += got;
            self.frames += got as u64;
        }
        Ok(written)
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let seconds = if seconds > 0.0 { seconds.min(self.duration()) } else { 0.0 };
        self.frames = ((seconds * f64::from(self.rate)) as u64).min(self.total_frames);
        Ok(())
    }

    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u32 {
        self.channels
    }
    fn duration(&self) -> f64 {
        self.total_frames as f64 / f64::from(self.rate)
    }
    fn position(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate)
    }
}

/// Plays one stream: pull samples with [`Player::render_i16`] or
/// [`Player::render_f32`] from your audio thread.
///
/// A player never touches an audio device and never allocates while rendering
/// (except the first call after a seek). It is `Send`, not `Sync`: use it from
/// one thread at a time.
///
/// # Examples
///
/// ```
/// use psx_audio::{MemorySource, Player, Source, XaStream};
///
/// let sector = psx_formats::fixtures::sector(1, 0, 0x04, 0x00);
/// let mut source = MemorySource::new();
/// source.add("STREAM.STR", &sector);
/// let blob = source.open("STREAM.STR").unwrap();
/// let player = Player::xa(blob, XaStream {
///     channel: 0,
///     first_sector: 0,
///     sector_count: 1,
///     duration: 0.0,
/// }).unwrap();
/// assert_eq!(player.channels(), 1);
/// ```
pub struct Player<'a> {
    engine: Box<dyn Engine + 'a>,
    gain_q12: i32,
    stopped: bool,
    scratch: Vec<i16>,
}

impl core::fmt::Debug for Player<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Player")
            .field("rate", &self.engine.rate())
            .field("channels", &self.engine.channels())
            .field("position", &self.engine.position())
            .finish_non_exhaustive()
    }
}

impl<'a> Player<'a> {
    /// Wrap `engine`.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{Engine, Player};
    ///
    /// struct Silence { frames: usize, rate: u32 }
    ///
    /// impl Engine for Silence {
    ///     fn render(&mut self, out: &mut [i16]) -> psx_audio::Result<usize> {
    ///         let frames = (out.len() / 2).min(self.frames);
    ///         out[..frames * 2].fill(0);
    ///         self.frames -= frames;
    ///         Ok(frames)
    ///     }
    ///     fn seek(&mut self, seconds: f64) -> psx_audio::Result<()> {
    ///         self.frames = (seconds * f64::from(self.rate)) as usize;
    ///         Ok(())
    ///     }
    ///     fn rate(&self) -> u32 { self.rate }
    ///     fn channels(&self) -> u32 { 2 }
    ///     fn duration(&self) -> f64 { 1.0 }
    ///     fn position(&self) -> f64 { 0.0 }
    /// }
    ///
    /// let mut player = Player::new(Silence { frames: 44_100, rate: 44_100 });
    /// let mut pcm = [0i16; 1024];
    /// assert_eq!(player.render_i16(&mut pcm)?, 512);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn new(engine: impl Engine + 'a) -> Self {
        Self { engine: Box::new(engine), gain_q12: GAIN_ONE, stopped: false, scratch: Vec::new() }
    }

    /// Play `stream` of an XA file through its channel decoder.
    ///
    /// # Errors
    ///
    /// Fails when the stream has no audio sector for `stream.channel`, or its sectors are
    /// malformed.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source, XaStream};
    ///
    /// // Two silent 4-bit mono sectors at 37.8 kHz.
    /// let sector = psx_formats::fixtures::sector(1, 0, 0x04, 0x00);
    /// let stream: Vec<u8> = sector.iter().copied().chain(sector.iter().copied()).collect();
    /// let mut source = MemorySource::new();
    /// source.add("STREAM.STR", &stream);
    /// let blob = source.open("STREAM.STR").unwrap();
    /// let mut player = Player::xa(blob, XaStream {
    ///     channel: 0,
    ///     first_sector: 0,
    ///     sector_count: 2,
    ///     duration: 0.0,
    /// }).unwrap();
    /// assert_eq!(player.sample_rate(), 37_800);
    /// ```
    pub fn xa(blob: Arc<dyn Blob + 'a>, stream: XaStream) -> Result<Self> {
        Ok(Self::new(XaEngine::new(blob, stream)?))
    }

    /// Play a CD-DA track: raw 2352-byte sectors when `raw`, a 16-bit WAVE file otherwise.
    ///
    /// # Errors
    ///
    /// Fails when the file is not 16-bit PCM, has no complete frame, or is truncated.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 8], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let player = Player::cdda(blob, false).unwrap();
    /// assert_eq!((player.sample_rate(), player.channels()), (44_100, 2));
    /// ```
    pub fn cdda(blob: Arc<dyn Blob + 'a>, raw: bool) -> Result<Self> {
        Ok(Self::new(CddaEngine::new(blob, raw)?))
    }

    /// Sample rate of the produced audio, in Hz. The player never resamples.
    #[must_use]
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// assert_eq!(player.sample_rate(), 44_100);
    /// ```
    pub fn sample_rate(&self) -> u32 {
        self.engine.rate()
    }

    /// Interleaved channels per frame (1 or 2).
    #[must_use]
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// assert_eq!(player.channels(), 2);
    /// ```
    pub fn channels(&self) -> u32 {
        self.engine.channels()
    }

    /// Length of one pass in seconds (music includes its release tail).
    #[must_use]
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// assert_eq!(player.duration(), 2.0 / 44_100.0);
    /// ```
    pub fn duration(&self) -> f64 {
        self.engine.duration()
    }

    /// Current position in seconds.
    #[must_use]
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// assert_eq!(player.position(), 0.0);
    /// ```
    pub fn position(&self) -> f64 {
        self.engine.position()
    }

    /// The stream's own loop window `(start, end)` in seconds, when it carries one.
    #[must_use]
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// assert_eq!(player.loop_window(), None);
    /// ```
    pub fn loop_window(&self) -> Option<(f64, f64)> {
        self.engine.loop_window()
    }

    /// Repeat the loop window (or the whole stream when it has none).
    /// Engines without a loop window ignore this.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// player.set_loop(psx_audio::Loop::Forever);
    /// ```
    pub fn set_loop(&mut self, mode: Loop) {
        self.engine.set_loop(mode);
    }

    /// Output gain, `0.0..=4.0`; 1.0 is a bit-exact bypass.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// player.set_gain(0.5);
    /// ```
    pub fn set_gain(&mut self, gain: f32) {
        let gain = if gain >= 0.0 { gain.min(GAIN_MAX) } else { 0.0 };
        self.gain_q12 = (gain * GAIN_ONE as f32 + 0.5) as i32;
    }

    /// Stop: further renders return 0 frames until [`Player::seek`] or [`Player::play`].
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// player.stop();
    /// assert_eq!(player.render_i16(&mut [0i16; 4]).unwrap(), 0);
    /// ```
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Undo [`Player::stop`] without moving the position.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// player.stop();
    /// player.play();
    /// ```
    pub fn play(&mut self) {
        self.stopped = false;
    }

    /// Move to `seconds`, clamped to the stream. Seeking also resumes a stopped player.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// player.seek(0.0)?;
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        self.stopped = false;
        self.engine.seek(seconds)
    }

    /// Render interleaved 16-bit PCM; returns the frames written (0 at the end).
    ///
    /// `out.len()` must be a multiple of [`Player::channels`].
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// let mut pcm = [0i16; 4];
    /// assert_eq!(player.render_i16(&mut pcm)?, 2);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn render_i16(&mut self, out: &mut [i16]) -> Result<usize> {
        let channels = self.engine.channels() as usize;
        if !out.len().is_multiple_of(channels) {
            return Err(Error::invalid("buffer must hold whole audio frames"));
        }
        if self.stopped {
            return Ok(0);
        }
        let frames = self.engine.render(out)?;
        self.apply_gain(&mut out[..frames * channels]);
        Ok(frames)
    }

    /// Render interleaved 32-bit float PCM in `-1.0..1.0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// let mut pcm = [0.0f32; 4];
    /// assert_eq!(player.render_f32(&mut pcm)?, 2);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn render_f32(&mut self, out: &mut [f32]) -> Result<usize> {
        let mut scratch = core::mem::take(&mut self.scratch);
        scratch.resize(out.len(), 0);
        let result = self.render_i16(&mut scratch);
        if let Ok(frames) = &result {
            let samples = frames * self.engine.channels() as usize;
            for (dst, src) in out.iter_mut().zip(&scratch[..samples]) {
                *dst = f32::from(*src) / 32768.0;
            }
        }
        self.scratch = scratch;
        result
    }

    /// Render the rest of the stream. Never call this on a forever-looping player.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::{MemorySource, Player, Source};
    ///
    /// let wav = psx_formats::wav::pcm(&[0i16; 4], 44_100, 2).unwrap();
    /// let mut source = MemorySource::new();
    /// source.add("CDDA/TRACK02.WAV", &wav);
    /// let blob = source.open("CDDA/TRACK02.WAV").unwrap();
    /// let mut player = Player::cdda(blob, false).unwrap();
    ///
    /// assert_eq!(player.render_all()?.len(), 4);
    /// # Ok::<(), psx_audio::Error>(())
    /// ```
    pub fn render_all(&mut self) -> Result<Vec<i16>> {
        let channels = self.engine.channels() as usize;
        let mut pcm = Vec::new();
        let mut chunk = alloc::vec![0i16; 4096 * channels];
        loop {
            let frames = self.render_i16(&mut chunk)?;
            if frames == 0 {
                return Ok(pcm);
            }
            pcm.extend_from_slice(&chunk[..frames * channels]);
        }
    }

    fn apply_gain(&self, samples: &mut [i16]) {
        if self.gain_q12 == GAIN_ONE {
            return;
        }
        for sample in samples {
            let scaled = (i32::from(*sample) * self.gain_q12) >> GAIN_BITS;
            *sample = scaled.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
    }
}
