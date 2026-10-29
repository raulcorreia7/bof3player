// SPDX-License-Identifier: MIT
//! Audio output: a [`Deck`] owns the cpal stream and pulls PCM from one player.
//!
//! The library never resamples, so the deck adapts the item's rate and channel
//! count to the device with a linear resampler. The audio callback shares a
//! small state block with the UI thread; the UI only holds the lock briefly.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::Player;
use crate::player::Loop;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// A position as `minutes:seconds` with one decimal, for example `1:05.5`.
#[must_use]
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0);
    format!("{}:{:04.1}", (total / 60.0) as u64, total % 60.0)
}

/// Frames pulled from the player per refill.
const CHUNK_FRAMES: usize = 1024;
/// Samples kept for the spectrum analysis.
pub const SCOPE_SAMPLES: usize = 4096;
/// Pace of the silent output used by `--mute`.
const MUTE_BLOCK_FRAMES: usize = 512;

/// Anything the deck can play: a library item's player or a decoded WAV file.
pub trait Source: Send {
    /// Frames per second of the rendered samples.
    fn sample_rate(&self) -> u32;
    /// Interleaved channels per frame (1 or 2).
    fn channels(&self) -> usize;
    /// Playback position in seconds.
    fn position(&self) -> f64;
    /// Length in seconds.
    fn duration(&self) -> f64;
    /// The loop window in seconds, when the source has one.
    fn loop_window(&self) -> Option<(f64, f64)>;
    /// Set how often the loop window repeats.
    fn set_loop(&mut self, mode: Loop);
    /// Move to `seconds`.
    fn seek(&mut self, seconds: f64);
    /// Fill `out` with interleaved samples; returns the frames written (0 at the end).
    fn render_f32(&mut self, out: &mut [f32]) -> usize;
}

impl Source for Player<'static> {
    fn sample_rate(&self) -> u32 {
        Player::sample_rate(self)
    }
    fn channels(&self) -> usize {
        Player::channels(self) as usize
    }
    fn position(&self) -> f64 {
        Player::position(self)
    }
    fn duration(&self) -> f64 {
        Player::duration(self)
    }
    fn loop_window(&self) -> Option<(f64, f64)> {
        Player::loop_window(self)
    }
    fn set_loop(&mut self, mode: Loop) {
        Player::set_loop(self, mode);
    }
    fn seek(&mut self, seconds: f64) {
        let _ = Player::seek(self, seconds);
    }
    fn render_f32(&mut self, out: &mut [f32]) -> usize {
        Player::render_f32(self, out).unwrap_or(0)
    }
}

/// What the audio thread needs to know about the item.
struct Inner {
    player: Option<Box<dyn Source>>,
    src_rate: u32,
    src_channels: usize,
    paused: bool,
    ended: bool,
    chunk: Vec<f32>,
    chunk_pos: usize,
    chunk_len: usize,
    prev: [f32; 2],
    cur: [f32; 2],
    frac: f64,
    scope: VecDeque<f32>,
}

impl Inner {
    fn reset_stream(&mut self) {
        self.chunk_pos = 0;
        self.chunk_len = 0;
        self.prev = [0.0; 2];
        self.cur = [0.0; 2];
        self.frac = 0.0;
        self.ended = false;
    }

    /// Next source frame as stereo.
    fn next_frame(&mut self) -> [f32; 2] {
        if self.chunk_pos >= self.chunk_len {
            let want = CHUNK_FRAMES * self.src_channels;
            self.chunk.resize(want, 0.0);
            let frames = self.player.as_mut().map_or(0, |p| p.render_f32(&mut self.chunk));
            self.chunk_len = frames * self.src_channels;
            self.chunk_pos = 0;
            if frames == 0 {
                self.ended = true;
                return [0.0; 2];
            }
        }
        let at = self.chunk_pos;
        self.chunk_pos += self.src_channels;
        if self.src_channels == 1 {
            let v = self.chunk[at];
            [v, v]
        } else {
            [self.chunk[at], self.chunk[at + 1]]
        }
    }
}

/// State shared with the audio callback.
pub struct Shared {
    inner: Mutex<Inner>,
    volume: AtomicU32,
    muted: AtomicBool,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    finished: AtomicBool,
    /// The first stream error that is not an underrun/overrun.
    error: Mutex<Option<String>>,
    /// Stream callback reports of an underrun or overrun.
    xruns: AtomicU64,
    dev_rate: u32,
    dev_channels: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keep the first non-xrun stream error for the UI; count xruns instead of
    /// storing them. The stream stays open: cpal keeps delivering callbacks,
    /// and a rebuild would drop queued audio on every underrun.
    fn record_error(&self, error: &cpal::Error) {
        if error.kind() == cpal::ErrorKind::Xrun {
            self.xruns.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut slot = self.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(error.to_string());
        }
    }

    /// Fill `out` (interleaved, `dev_channels` per frame).
    fn fill(&self, out: &mut [f32]) {
        let mut inner = self.lock();
        let channels = self.dev_channels;
        out.fill(0.0);
        if inner.player.is_none() || inner.paused || inner.ended {
            self.peak_left.store(0, Ordering::Relaxed);
            self.peak_right.store(0, Ordering::Relaxed);
            return;
        }
        let volume =
            if self.muted.load(Ordering::Relaxed) { 0.0 } else { f32::from_bits(self.volume.load(Ordering::Relaxed)) };
        let ratio = f64::from(inner.src_rate) / f64::from(self.dev_rate);
        let (mut peak_l, mut peak_r) = (0.0f32, 0.0f32);
        for frame in out.chunks_exact_mut(channels) {
            let [l, r] = if (ratio - 1.0).abs() < 1e-9 {
                inner.next_frame()
            } else {
                while inner.frac >= 1.0 {
                    inner.prev = inner.cur;
                    inner.cur = inner.next_frame();
                    inner.frac -= 1.0;
                }
                let t = inner.frac as f32;
                inner.frac += ratio;
                [inner.prev[0] + (inner.cur[0] - inner.prev[0]) * t, inner.prev[1] + (inner.cur[1] - inner.prev[1]) * t]
            };
            // The analyzer and the meters tap the signal before volume and mute, so they keep
            // moving when the output is quiet or off.
            if inner.scope.len() >= SCOPE_SAMPLES {
                inner.scope.pop_front();
            }
            inner.scope.push_back((l + r) * 0.5);
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
            let (l, r) = (l * volume, r * volume);
            if channels == 1 {
                frame[0] = (l + r) * 0.5;
            } else {
                frame[0] = l;
                frame[1] = r;
            }
            if inner.ended {
                break;
            }
        }
        self.peak_left.store(peak_l.to_bits(), Ordering::Relaxed);
        self.peak_right.store(peak_r.to_bits(), Ordering::Relaxed);
        if inner.ended {
            self.finished.store(true, Ordering::Relaxed);
        }
    }
}

/// The stream error callback: records the error for the UI instead of writing
/// over the terminal with `eprintln!`.
fn error_sink(shared: Arc<Shared>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |error| shared.record_error(&error)
}

/// An output stream and the player it pulls from.
pub struct Deck {
    shared: Arc<Shared>,
    _stream: Option<cpal::Stream>,
    /// Name of the output device, or `silent`.
    pub device: String,
    /// The scope copy [`analyze`](Self::analyze) reuses, so it does not allocate per frame.
    samples: Mutex<Vec<f32>>,
}

impl Deck {
    /// Open the default output device, or a silent real-time sink when no device can be opened
    /// (the reason is in `device`). With `mute` the sound starts off; [`set_muted`](Self::set_muted)
    /// turns it back on.
    pub fn new(mute: bool) -> Self {
        let deck = Self::open_device().unwrap_or_else(|why| {
            let mut deck = Self::silent();
            deck.device = format!("silent ({why})");
            deck
        });
        deck.set_muted(mute);
        deck
    }

    fn make_shared(rate: u32, channels: usize) -> Arc<Shared> {
        Arc::new(Shared {
            inner: Mutex::new(Inner {
                player: None,
                src_rate: rate,
                src_channels: 2,
                paused: false,
                ended: false,
                chunk: Vec::new(),
                chunk_pos: 0,
                chunk_len: 0,
                prev: [0.0; 2],
                cur: [0.0; 2],
                frac: 0.0,
                scope: VecDeque::new(),
            }),
            volume: AtomicU32::new(0.8f32.to_bits()),
            muted: AtomicBool::new(false),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            finished: AtomicBool::new(false),
            error: Mutex::new(None),
            xruns: AtomicU64::new(0),
            dev_rate: rate,
            dev_channels: channels,
        })
    }

    fn silent() -> Self {
        let shared = Self::make_shared(44_100, 2);
        let worker = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut block = vec![0.0f32; MUTE_BLOCK_FRAMES * 2];
            let pace = Duration::from_secs_f64(MUTE_BLOCK_FRAMES as f64 / 44_100.0);
            while Arc::strong_count(&worker) > 1 {
                worker.fill(&mut block);
                std::thread::sleep(pace);
            }
        });
        Self { shared, _stream: None, device: "silent".into(), samples: Mutex::default() }
    }

    fn open_device() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no output device")?;
        let name = device.description().map_or_else(|_| "default output".to_string(), |d| d.to_string());
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let shared = Self::make_shared(config.sample_rate(), usize::from(config.channels()).max(1));
        let mut stream_config = config.config();
        stream_config.buffer_size = match config.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } if *max >= 2048 => {
                cpal::BufferSize::Fixed(2048u32.clamp(*min, *max))
            }
            _ => cpal::BufferSize::Default,
        };
        let worker = Arc::clone(&shared);
        let on_error = error_sink(Arc::clone(&shared));
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| worker.fill(data),
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let mut scratch = Vec::new();
                device.build_output_stream(
                    stream_config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        scratch.resize(data.len(), 0.0);
                        worker.fill(&mut scratch);
                        for (d, s) in data.iter_mut().zip(&scratch) {
                            *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                    },
                    on_error,
                    None,
                )
            }
            other => return Err(format!("unsupported device sample format {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { shared, _stream: Some(stream), device: name, samples: Mutex::default() })
    }

    /// Start playing `player` from its current position.
    pub fn load(&self, player: impl Source + 'static) {
        let mut inner = self.shared.lock();
        inner.src_rate = player.sample_rate();
        inner.src_channels = player.channels();
        inner.player = Some(Box::new(player));
        inner.paused = false;
        inner.scope.clear();
        inner.reset_stream();
        self.shared.finished.store(false, Ordering::Relaxed);
    }

    /// Drop the current player.
    pub fn unload(&self) {
        let mut inner = self.shared.lock();
        inner.player = None;
        inner.reset_stream();
        self.shared.finished.store(false, Ordering::Relaxed);
    }

    /// Whether an item is loaded.
    pub fn is_loaded(&self) -> bool {
        self.shared.lock().player.is_some()
    }

    /// Pause or resume.
    pub fn set_paused(&self, paused: bool) {
        self.shared.lock().paused = paused;
    }

    /// Whether playback is paused.
    pub fn paused(&self) -> bool {
        self.shared.lock().paused
    }

    /// The item ended since it was loaded.
    pub fn finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed)
    }

    /// The first stream error since the last call, cleared.
    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }

    /// Position and duration in seconds.
    pub fn times(&self) -> (f64, f64) {
        self.shared.lock().player.as_ref().map_or((0.0, 0.0), |p| (p.position(), p.duration()))
    }

    /// The item's loop window.
    pub fn loop_window(&self) -> Option<(f64, f64)> {
        self.shared.lock().player.as_ref().and_then(|p| p.loop_window())
    }

    /// Set the loop mode of the loaded source.
    pub fn set_loop(&self, mode: Loop) {
        if let Some(player) = self.shared.lock().player.as_mut() {
            player.set_loop(mode);
        }
    }

    /// Seek to `seconds`.
    pub fn seek(&self, seconds: f64) {
        let mut inner = self.shared.lock();
        if let Some(player) = inner.player.as_mut() {
            player.seek(seconds);
            inner.reset_stream();
            self.shared.finished.store(false, Ordering::Relaxed);
        }
    }

    /// Output volume, `0.0..=1.5`.
    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.store(volume.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed);
    }

    /// Silence the output without changing the volume, or restore it.
    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    /// Whether the output is silenced.
    pub fn muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    /// Output volume.
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    /// Recent peak levels (left, right), `0.0..`, before volume and mute.
    pub fn peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.peak_left.load(Ordering::Relaxed)),
            f32::from_bits(self.shared.peak_right.load(Ordering::Relaxed)),
        )
    }

    /// Frames per second of the output device, the rate of [`Deck::scope`].
    pub fn output_rate(&self) -> u32 {
        self.shared.dev_rate
    }

    /// Update `spectrum` from the output, or let its bars fall while nothing plays.
    pub fn analyze(&self, spectrum: &mut crate::spectrum::Spectrum) {
        if self.is_loaded() && !self.paused() {
            let mut samples = self.samples.lock().unwrap_or_else(PoisonError::into_inner);
            samples.clear();
            samples.extend(&self.shared.lock().scope);
            spectrum.update(&samples, self.output_rate());
        } else {
            spectrum.fade();
        }
    }

    /// The last output samples (mono mix), oldest first.
    pub fn scope(&self) -> Vec<f32> {
        self.shared.lock().scope.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::Spectrum;

    /// A looping 1 kHz tone at 44.1 kHz.
    struct Tone(usize);

    impl Source for Tone {
        fn sample_rate(&self) -> u32 {
            44_100
        }
        fn channels(&self) -> usize {
            1
        }
        fn position(&self) -> f64 {
            0.0
        }
        fn duration(&self) -> f64 {
            1.0
        }
        fn loop_window(&self) -> Option<(f64, f64)> {
            None
        }
        fn set_loop(&mut self, _: Loop) {}
        fn seek(&mut self, _: f64) {}
        fn render_f32(&mut self, out: &mut [f32]) -> usize {
            for sample in out.iter_mut() {
                *sample = (std::f32::consts::TAU * 1000.0 * self.0 as f32 / 44_100.0).sin() * 0.8;
                self.0 += 1;
            }
            out.len()
        }
    }

    #[test]
    fn the_spectrum_and_meters_move_while_muted() {
        let shared = Deck::make_shared(44_100, 2);
        {
            let mut inner = shared.lock();
            inner.player = Some(Box::new(Tone(0)));
            inner.src_channels = 1;
        }
        shared.muted.store(true, Ordering::Relaxed);
        let mut out = vec![0.0f32; 8192 * 2];
        shared.fill(&mut out);
        assert!(out.iter().all(|&sample| sample == 0.0), "the output is silent");
        assert!(f32::from_bits(shared.peak_left.load(Ordering::Relaxed)) > 0.1, "the meters move");
        let scope: Vec<f32> = shared.lock().scope.iter().copied().collect();
        let mut spectrum = Spectrum::new(48);
        spectrum.update(&scope, 44_100);
        assert!(spectrum.levels.iter().any(|&level| level > 0.1), "{:?}", spectrum.levels);
    }
}
