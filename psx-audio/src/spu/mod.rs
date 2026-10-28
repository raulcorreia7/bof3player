// SPDX-License-Identifier: MIT
//! The SPU voice engine: 24 ADPCM voices, ADSR, 4-point Gaussian
//! interpolation, reverb and the main mixer, rendered at 44.1 kHz.
//!
//! Everything here follows the public hardware description in psx-spx
//! (third_party/references/psx-spx.txt, chapter "Sound Processing Unit (SPU)"). Not modelled:
//! noise generator, pitch modulation, volume sweep, capture buffers and CD
//! audio input.
//!
//! # Examples
//!
//! ```
//! use psx_audio::spu::{PITCH_ONE, Spu, VoiceSetup};
//!
//! let mut spu = Spu::new();
//! spu.upload(0, &[0x00; 32]);
//! spu.set_main_volume(0x3FFF, 0x3FFF);
//! spu.key_on(0, &VoiceSetup {
//!     pitch: PITCH_ONE as u16,
//!     adsr1: 0x00F5,
//!     adsr2: 0x0040,
//!     volume_left: 0x3FFF,
//!     volume_right: 0x3FFF,
//!     ..VoiceSetup::default()
//! });
//! let mut pcm = [0i16; 2];
//! spu.render(&mut pcm);
//! assert!(spu.voice_active(0));
//! ```

mod envelope;
mod gauss;
mod pitch_table;
mod reverb;
mod tables;

use alloc::boxed::Box;
use alloc::vec;

use envelope::{Envelope, Phase};
use gauss::GAUSS;
pub use pitch_table::PITCH_TABLE;
use psx_formats::adpcm;
use psx_formats::adpcm::{BLOCK_BYTES, BLOCK_SAMPLES, State};
use reverb::Reverb;
pub use reverb::ReverbModel;
pub use tables::{PRESET_ROOM, REVERB_PRESETS, ReverbPreset};

/// How long each ADSR phase of a voice takes, in seconds.
///
/// # Examples
///
/// ```
/// let timing = psx_audio::spu::envelope_timing(0x00F5, 0x0040).expect("the phases end");
/// assert!(timing.attack > 0.0 && timing.release > 0.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvelopeTiming {
    /// From key-on to full scale.
    pub attack: f64,
    /// From full scale to the sustain level.
    pub decay: f64,
    /// The sustain level, `(N + 1) * 0x800`.
    pub sustain_level: i32,
    /// From the sustain level to silence after key-off.
    pub release: f64,
}

/// Time each ADSR phase of the voice with these `ADSR1`/`ADSR2` registers.
///
/// `None` when a phase never ends. The SoundFont export needs this; the renderer itself
/// steps the envelope per sample.
///
/// # Examples
///
/// ```
/// let timing = psx_audio::spu::envelope_timing(0x00F5, 0x0040).unwrap();
/// assert_eq!(timing.sustain_level, (0x05 + 1) * 0x800);
/// ```
#[must_use]
pub fn envelope_timing(adsr1: u16, adsr2: u16) -> Option<EnvelopeTiming> {
    let timing = envelope::timing_of(adsr1, adsr2)?;
    let seconds = |samples: u64| samples as f64 / f64::from(SAMPLE_RATE);
    Some(EnvelopeTiming {
        attack: seconds(timing.attack),
        decay: seconds(timing.decay),
        sustain_level: timing.sustain_level,
        release: seconds(timing.release),
    })
}

/// Size of SPU RAM in bytes (512 KiB).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::RAM_SIZE, 512 * 1024);
/// ```
pub const RAM_SIZE: usize = 512 * 1024;

/// Number of hardware voices.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::VOICE_COUNT, 24);
/// ```
pub const VOICE_COUNT: usize = 24;

/// Output sample rate in Hz.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::SAMPLE_RATE, 44_100);
/// ```
pub const SAMPLE_RATE: u32 = 44_100;

/// `VxPitch` value that plays a sample at its recorded rate (psx-spx: "1000h = 44100Hz").
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::PITCH_ONE, 0x1000);
/// ```
pub const PITCH_ONE: u32 = 0x1000;

/// Largest effective `VxPitch` (psx-spx: values above 4000h act as 4000h).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::PITCH_MAX, 0x4000);
/// ```
pub const PITCH_MAX: u32 = 0x4000;

/// Address registers hold byte addresses divided by this.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::ADDRESS_UNIT, 8);
/// ```
pub const ADDRESS_UNIT: u32 = 8;

/// Default start of the reverb work area: the top 64 KiB of SPU RAM.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::REVERB_AREA_START, 0x70000);
/// ```
pub const REVERB_AREA_START: u32 = 0x70000;

/// Pitch counter bits below the sample index.
const COUNTER_SAMPLE_SHIFT: u32 = 12;
/// Bits of the counter used as the Gaussian index sit at 4..=11.
const GAUSS_INDEX_SHIFT: u32 = 4;
const GAUSS_INDEX_MASK: u32 = 0xFF;
/// Fixed-point fraction bits of volumes, envelope and Gaussian weights.
const FRACTION_BITS: u32 = 15;
/// A fixed-mode volume register holds half the volume (psx-spx: "Voice volume/2").
const VOLUME_DOUBLE: u32 = 1;
/// Fixed volume mode keeps 15 bits; bit 15 selects sweep mode (not modelled).
const VOLUME_BITS_MASK: u16 = 0x7FFF;
const VOLUME_SIGN_BIT: u16 = 0x4000;

/// Everything a key-on needs, in the hardware's own register terms.
///
/// # Examples
///
/// ```
/// use psx_audio::spu::{PITCH_ONE, VoiceSetup};
///
/// let setup = VoiceSetup {
///     pitch: PITCH_ONE as u16,
///     adsr1: 0x00F5,
///     adsr2: 0x0040,
///     volume_left: 0x3FFF,
///     volume_right: 0x3FFF,
///     ..VoiceSetup::default()
/// };
/// assert_eq!(setup.start, 0);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct VoiceSetup {
    /// Sample start, bytes in SPU RAM.
    pub start: u32,
    /// Loop target, bytes in SPU RAM.
    pub repeat: u32,
    /// `VxPitch` register.
    pub pitch: u16,
    /// `ADSR1` register (low half of the envelope word).
    pub adsr1: u16,
    /// `ADSR2` register.
    pub adsr2: u16,
    /// Left volume register (fixed mode, 15 bits).
    pub volume_left: u16,
    /// Right volume register.
    pub volume_right: u16,
    /// Also feed the reverb unit.
    pub reverb_send: bool,
}

#[derive(Debug, Clone)]
struct Voice {
    envelope: Envelope,
    pitch: u16,
    volume_left: u16,
    volume_right: u16,
    reverb_send: bool,
    address: u32,
    repeat: u32,
    counter: u32,
    pushed: usize,
    block: [i16; BLOCK_SAMPLES],
    flags: u8,
    adpcm: State,
    history: [i16; 4],
    ended: bool,
}

impl Voice {
    const fn idle() -> Self {
        Self {
            envelope: Envelope::off(),
            pitch: 0,
            volume_left: 0,
            volume_right: 0,
            reverb_send: false,
            address: 0,
            repeat: 0,
            counter: 0,
            pushed: 0,
            block: [0; BLOCK_SAMPLES],
            flags: 0,
            adpcm: State { prev1: 0, prev2: 0 },
            history: [0; 4],
            ended: false,
        }
    }

    fn active(&self) -> bool {
        self.envelope.phase != Phase::Off
    }
}

/// Signed value of a fixed-mode volume register, on the 16-bit scale.
fn volume_value(register: u16) -> i32 {
    let mut value = i32::from(register & VOLUME_BITS_MASK);
    if register & VOLUME_SIGN_BIT != 0 {
        value -= 0x8000;
    }
    value << VOLUME_DOUBLE
}

/// The SPU: RAM, voices, reverb and the main volume.
///
/// # Examples
///
/// ```
/// let mut spu = psx_audio::spu::Spu::new();
/// let mut pcm = [0i16; 4];
/// spu.render(&mut pcm);
/// assert_eq!(pcm, [0, 0, 0, 0]);
/// ```
pub struct Spu {
    ram: Box<[u8]>,
    voices: [Voice; VOICE_COUNT],
    main_left: u16,
    main_right: u16,
    reverb: Reverb,
}

impl Default for Spu {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Spu {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Spu").field("active_voices", &self.active_voices()).finish_non_exhaustive()
    }
}

impl Spu {
    /// A silent SPU with zeroed RAM.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_audio::spu::Spu::new().active_voices(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::with_reverb_model(ReverbModel::Hardware)
    }

    /// A silent SPU whose reverb unit uses the given delay-line timing.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::spu::{ReverbModel, Spu};
    ///
    /// let spu = Spu::with_reverb_model(ReverbModel::Legacy);
    /// assert_eq!(spu.active_voices(), 0);
    /// ```
    #[must_use]
    pub fn with_reverb_model(model: ReverbModel) -> Self {
        let mut reverb = Reverb::new(REVERB_AREA_START);
        reverb.set_model(model);
        Self {
            ram: vec![0u8; RAM_SIZE].into_boxed_slice(),
            voices: core::array::from_fn(|_| Voice::idle()),
            main_left: 0,
            main_right: 0,
            reverb,
        }
    }

    /// Copy `data` into SPU RAM at byte `address` (wraps at the end of RAM).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut spu = psx_audio::spu::Spu::new();
    /// spu.upload(psx_audio::spu::RAM_SIZE as u32 - 2, &[0x22, 0x22]);
    /// ```
    pub fn upload(&mut self, address: u32, data: &[u8]) {
        let mut at = address as usize % RAM_SIZE;
        for &byte in data {
            self.ram[at] = byte;
            at = (at + 1) % RAM_SIZE;
        }
    }

    /// Set the main volume registers (fixed mode).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut spu = psx_audio::spu::Spu::new();
    /// spu.set_main_volume(0x3FFF, 0x3FFF);
    /// ```
    pub fn set_main_volume(&mut self, left: u16, right: u16) {
        self.main_left = left;
        self.main_right = right;
    }

    /// Load reverb `preset`, place the work area at `base` bytes, set the output
    /// volume registers and enable buffer writes.
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::spu::{PRESET_ROOM, REVERB_AREA_START, REVERB_PRESETS, Spu};
    ///
    /// let mut spu = Spu::new();
    /// spu.configure_reverb(&REVERB_PRESETS[PRESET_ROOM], REVERB_AREA_START, 0x7FFF, 0x7FFF);
    /// ```
    pub fn configure_reverb(&mut self, preset: &ReverbPreset, base: u32, left: u16, right: u16) {
        self.reverb.load_preset(preset);
        self.reverb.set_base(base);
        self.reverb.out_left = left as i16;
        self.reverb.out_right = right as i16;
        self.reverb.enabled = true;
    }

    /// Start a voice from the beginning of its sample.
    ///
    /// # Panics
    ///
    /// Panics when `voice` is at least [`VOICE_COUNT`].
    ///
    /// # Examples
    ///
    /// ```
    /// use psx_audio::spu::{PITCH_ONE, Spu, VoiceSetup};
    ///
    /// let mut spu = Spu::new();
    /// spu.key_on(0, &VoiceSetup {
    ///     pitch: PITCH_ONE as u16,
    ///     adsr1: 0x00F5,
    ///     adsr2: 0x0040,
    ///     ..VoiceSetup::default()
    /// });
    /// assert!(spu.voice_active(0));
    /// ```
    pub fn key_on(&mut self, voice: usize, setup: &VoiceSetup) {
        let mut v = Voice::idle();
        v.envelope = Envelope::start(setup.adsr1, setup.adsr2);
        v.pitch = setup.pitch;
        v.volume_left = setup.volume_left;
        v.volume_right = setup.volume_right;
        v.reverb_send = setup.reverb_send;
        v.address = setup.start % RAM_SIZE as u32;
        v.repeat = setup.repeat % RAM_SIZE as u32;
        self.voices[voice] = v;
        self.decode_block(voice);
    }

    /// Begin the release phase of a voice.
    ///
    /// # Panics
    ///
    /// Panics when `voice` is at least [`VOICE_COUNT`].
    ///
    /// # Examples
    ///
    /// ```
    /// let mut spu = psx_audio::spu::Spu::new();
    /// spu.key_off(0);
    /// ```
    pub fn key_off(&mut self, voice: usize) {
        self.voices[voice].envelope.release();
    }

    /// Change a playing voice's volume registers.
    ///
    /// # Panics
    ///
    /// Panics when `voice` is at least [`VOICE_COUNT`].
    ///
    /// # Examples
    ///
    /// ```
    /// let mut spu = psx_audio::spu::Spu::new();
    /// spu.set_voice_volume(0, 0x3FFF, 0x3FFF);
    /// ```
    pub fn set_voice_volume(&mut self, voice: usize, left: u16, right: u16) {
        self.voices[voice].volume_left = left;
        self.voices[voice].volume_right = right;
    }

    /// Change a playing voice's pitch register.
    ///
    /// # Panics
    ///
    /// Panics when `voice` is at least [`VOICE_COUNT`].
    ///
    /// # Examples
    ///
    /// ```
    /// let mut spu = psx_audio::spu::Spu::new();
    /// spu.set_voice_pitch(0, psx_audio::spu::PITCH_ONE as u16);
    /// ```
    pub fn set_voice_pitch(&mut self, voice: usize, pitch: u16) {
        self.voices[voice].pitch = pitch;
    }

    /// Whether the voice is still sounding (not finished, not released to zero).
    ///
    /// # Panics
    ///
    /// Panics when `voice` is at least [`VOICE_COUNT`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!psx_audio::spu::Spu::new().voice_active(0));
    /// ```
    #[must_use]
    pub fn voice_active(&self, voice: usize) -> bool {
        self.voices[voice].active()
    }

    /// Current envelope level of a voice, `0..=0x7FFF`.
    ///
    /// # Panics
    ///
    /// Panics when `voice` is at least [`VOICE_COUNT`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_audio::spu::Spu::new().envelope_level(0), 0);
    /// ```
    #[must_use]
    pub fn envelope_level(&self, voice: usize) -> u16 {
        self.voices[voice].envelope.level as u16
    }

    /// Number of voices still sounding.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(psx_audio::spu::Spu::new().active_voices(), 0);
    /// ```
    #[must_use]
    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active()).count()
    }

    fn decode_block(&mut self, index: usize) {
        let v = &mut self.voices[index];
        let at = v.address as usize;
        let mut block = [0u8; BLOCK_BYTES];
        for (i, byte) in block.iter_mut().enumerate() {
            *byte = self.ram[(at + i) % RAM_SIZE];
        }
        adpcm::decode_block(&block, &mut v.block, &mut v.adpcm);
        v.flags = block[1];
        if v.flags & adpcm::FLAG_LOOP_START != 0 {
            v.repeat = v.address;
        }
        v.pushed = 0;
    }

    /// Leave the finished block: apply its end/repeat flags, decode the next.
    fn next_block(&mut self, index: usize) {
        let v = &mut self.voices[index];
        if v.flags & adpcm::FLAG_END != 0 {
            if v.flags & adpcm::FLAG_REPEAT != 0 {
                v.address = v.repeat;
            } else {
                // End + mute: release and force the level to zero.
                v.envelope = Envelope::off();
                v.ended = true;
                return;
            }
        } else {
            v.address = (v.address + BLOCK_BYTES as u32) % RAM_SIZE as u32;
        }
        self.decode_block(index);
    }

    /// Bring the voice's sample history up to the current pitch counter.
    fn advance_history(&mut self, index: usize) {
        loop {
            let v = &self.voices[index];
            let sample_index = (v.counter >> COUNTER_SAMPLE_SHIFT) as usize;
            if v.pushed > sample_index {
                return;
            }
            if v.pushed >= BLOCK_SAMPLES {
                self.voices[index].counter -= (BLOCK_SAMPLES as u32) << COUNTER_SAMPLE_SHIFT;
                self.next_block(index);
                if self.voices[index].ended {
                    return;
                }
                continue;
            }
            let v = &mut self.voices[index];
            v.history = [v.history[1], v.history[2], v.history[3], v.block[v.pushed]];
            v.pushed += 1;
        }
    }

    /// Render interleaved stereo into `out`; a trailing odd sample is left untouched.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut spu = psx_audio::spu::Spu::new();
    /// let mut pcm = [1i16; 4];
    /// spu.render(&mut pcm);
    /// assert_eq!(pcm, [0, 0, 0, 0]);
    /// ```
    pub fn render(&mut self, out: &mut [i16]) {
        for frame in out.as_chunks_mut::<2>().0 {
            let (left, right) = self.sample();
            frame[0] = left;
            frame[1] = right;
        }
    }

    /// One 44.1 kHz output frame.
    fn sample(&mut self) -> (i16, i16) {
        let (mut dry_left, mut dry_right) = (0i32, 0i32);
        let (mut send_left, mut send_right) = (0i32, 0i32);
        for index in 0..VOICE_COUNT {
            if !self.voices[index].active() {
                continue;
            }
            self.advance_history(index);
            let v = &mut self.voices[index];
            if v.ended {
                continue;
            }
            let fraction = ((v.counter >> GAUSS_INDEX_SHIFT) & GAUSS_INDEX_MASK) as usize;
            let h = &v.history;
            let mut s = (i32::from(GAUSS[0xFF - fraction]) * i32::from(h[0])) >> FRACTION_BITS;
            s += (i32::from(GAUSS[0x1FF - fraction]) * i32::from(h[1])) >> FRACTION_BITS;
            s += (i32::from(GAUSS[0x100 + fraction]) * i32::from(h[2])) >> FRACTION_BITS;
            s += (i32::from(GAUSS[fraction]) * i32::from(h[3])) >> FRACTION_BITS;
            s = (s * v.envelope.level) >> FRACTION_BITS;
            let l = (s * volume_value(v.volume_left)) >> FRACTION_BITS;
            let r = (s * volume_value(v.volume_right)) >> FRACTION_BITS;
            dry_left += l;
            dry_right += r;
            if v.reverb_send {
                send_left += l;
                send_right += r;
            }
            v.envelope.tick();
            let step = u32::from(v.pitch).min(PITCH_MAX);
            v.counter += step;
        }
        let (rev_left, rev_right) = self.reverb.process(&mut self.ram, send_left, send_right);
        let left = ((dry_left + rev_left) * volume_value(self.main_left)) >> FRACTION_BITS;
        let right = ((dry_right + rev_right) * volume_value(self.main_right)) >> FRACTION_BITS;
        (left.clamp(-0x8000, 0x7FFF) as i16, right.clamp(-0x8000, 0x7FFF) as i16)
    }
}
