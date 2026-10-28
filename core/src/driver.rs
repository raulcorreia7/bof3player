// SPDX-License-Identifier: MIT
//! The game's sound driver, re-created over the SPU model: SEP sequences
//! scheduled on the driver's 60 Hz clock, VAB tones keyed onto voices, and the
//! start-up state the game gives libsnd (master volume, ROOM reverb).
//!
//! The driver semantics (how a note selects tones, the volume and pan law, the
//! pitch table walk, voice stealing) come from the project's analysis of the
//! retail game (wiki: formats/audio); the hardware underneath is [`crate::spu`].

use alloc::format;
use alloc::vec::Vec;

use crate::clock;
use crate::error::{Error, Result};
use crate::format::VAB_SAMPLE_RATE;
use crate::format::emi::{Emi, EntryType};
use crate::format::sep::{Kind, Package as Sep, Sequence};
use crate::format::vab::{Bank as Vab, Tone, Vag};
use crate::player::Loop;
use crate::spu::{
    self, ADDRESS_UNIT, PITCH_MAX, PITCH_ONE, PITCH_TABLE, REVERB_AREA_START, ReverbModel, Spu, VOICE_COUNT, VoiceSetup,
};
use crate::spu::{PRESET_ROOM, REVERB_PRESETS};

/// Stereo output.
pub(crate) const CHANNELS: u32 = 2;
/// Seconds of release tail after the last event.
pub(crate) const TAIL_SECONDS: i64 = 2;
/// Mixer values travel as 7 bits.
const MIX_MAX: u8 = 127;
/// Centre of a 7-bit pan.
const MIX_CENTER: u8 = 64;
/// SPU volume register value at 7-bit full scale.
const MIX_FULL_SCALE: u32 = 0x3FFF;
/// Master volume the game's audio init sets (`SsSetMVol(0x7F, 0x7F)`).
const PRIMER_MASTER_VOLUME: u8 = 0x7F;
/// Reverb depth the game's audio init sets.
const PRIMER_REVERB_DEPTH: u8 = 0x28;
/// Frames of one seek pass.
const SEEK_CHUNK_FRAMES: usize = 2048;

/// The seek both engines share: reset when the target lies behind the cursor,
/// then render up to it in [`SEEK_CHUNK_FRAMES`] slices. Each engine supplies
/// its own reset (`rewind` or `start`).
macro_rules! seek_body {
    ($reset:ident) => {
        pub fn seek(&mut self, seconds: f64) -> Result<()> {
            let seconds = if seconds > 0.0 { seconds.min(self.duration()) } else { 0.0 };
            let target = (seconds * f64::from(VAB_SAMPLE_RATE)) as i64;
            if target < self.frame {
                self.$reset()?;
            }
            let mut scratch = core::mem::take(&mut self.scratch);
            while self.frame < target {
                let n = ((target - self.frame) as usize).min(SEEK_CHUNK_FRAMES);
                if self.pump(&mut scratch[..n * 2]) == 0 {
                    break;
                }
            }
            self.scratch = scratch;
            Ok(())
        }
    };
}

/// Coarse bend runs 0..127 around the centre; fine pitch is in 1/128 semitone.
const BEND_DOWN_STEPS: i32 = MIX_CENTER as i32;
const BEND_UP_STEPS: i32 = (MIX_MAX - MIX_CENTER) as i32;
const FINE_PER_SEMITONE: i32 = 128;
const FINE_PER_STEP: i32 = FINE_PER_SEMITONE / BEND_DOWN_STEPS;

/// Pitch table geometry: 16 fine steps per semitone, 8 fine units per step.
const FINE_STEPS_PER_SEMITONE: i32 = 16;
const FINE_UNITS_PER_STEP: i32 = 8;
const SEMITONES_PER_OCTAVE: i32 = 12;
/// The table's 0x1000 row sits at note 60, octave 5.
const PITCH_REFERENCE_NOTE: i32 = 60;
const PITCH_REFERENCE_OCTAVE: i32 = 5;
const OCTAVE_SHIFT_LIMIT: i32 = 20;

/// Fallback envelope for a sample no tone names: fastest everything.
const DEFAULT_ADSR1: u16 = 0x000F;
const DEFAULT_ADSR2: u16 = 0x000F;

/// `VxPitch` for a note (PsyQ `libsnd` note walk over the pitch table).
///
/// `center` is the sample's centre note, `shift` the tone's fine tune, `fine`
/// is in 1/128 semitone. Clamped to [`PITCH_MAX`].
pub(crate) fn pitch_from_note(note: i32, fine: i32, center: i32, shift: i32) -> u16 {
    let mut fine_index = i32::from(fine as u16) + shift;
    let mut semitone = 0;
    fine_index /= FINE_UNITS_PER_STEP;
    if fine_index >= FINE_STEPS_PER_SEMITONE {
        semitone = 1;
        fine_index -= FINE_STEPS_PER_SEMITONE;
    }
    fine_index = fine_index.min(FINE_STEPS_PER_SEMITONE - 1);
    semitone += note - (center - PITCH_REFERENCE_NOTE);
    let row = semitone.rem_euclid(SEMITONES_PER_OCTAVE);
    let mut pitch = u32::from(PITCH_TABLE[(row * FINE_STEPS_PER_SEMITONE + fine_index) as usize]);
    let octave = semitone.div_euclid(SEMITONES_PER_OCTAVE) - PITCH_REFERENCE_OCTAVE;
    if octave > 0 {
        pitch <<= octave.min(OCTAVE_SHIFT_LIMIT);
    } else if octave < 0 {
        pitch >>= (-octave).min(OCTAVE_SHIFT_LIMIT);
    }
    pitch.min(PITCH_MAX) as u16
}

/// A 7-bit primer volume on the SPU's 15-bit register scale.
fn scale_7bit(value: u8) -> u16 {
    (u32::from(value) * MIX_FULL_SCALE / u32::from(MIX_MAX)) as u16
}

/// Voice volume register for a 0..1 gain: full scale at 1, squared below it.
fn fixed_voice_volume(gain: f32) -> u16 {
    if gain.is_nan() || gain <= 0.0 {
        0
    } else if gain >= 1.0 {
        MIX_FULL_SCALE as u16
    } else {
        (gain * gain * MIX_FULL_SCALE as f32) as u16
    }
}

/// Pan gains: full volume on the near side of the centre, falling one step per unit on the far side.
fn pan_gains(pan: i32) -> (f32, f32) {
    let pan = pan.clamp(0, i32::from(MIX_MAX));
    let centre = f32::from(MIX_CENTER);
    let left = if pan < i32::from(MIX_CENTER) { 1.0 } else { (i32::from(MIX_MAX) - pan) as f32 / centre };
    let right = if pan > i32::from(MIX_CENTER) { 1.0 } else { pan as f32 / centre };
    (left, right)
}

/// Bank master, program and tone volume as fractions of the 7-bit scale.
fn tone_volume(master: u8, tone: &Tone) -> f32 {
    let scale = f32::from(MIX_MAX);
    (f32::from(master) / scale) * (f32::from(tone.program_vol) / scale) * (f32::from(tone.vol) / scale)
}

/// Pan a tone adds to its bank and program pans.
fn tone_pan(master_pan: u8, tone: &Tone) -> i32 {
    (i32::from(tone.pan)
        + (i32::from(tone.program_pan) - i32::from(MIX_CENTER))
        + (i32::from(master_pan) - i32::from(MIX_CENTER)))
    .clamp(0, i32::from(MIX_MAX))
}

/// The game's audio-init state plus `body` staged at the bottom of SPU RAM.
pub(crate) fn configure_spu(spu: &mut Spu, body: &[u8]) -> Result<()> {
    if body.len() > spu::RAM_SIZE {
        return Err(Error::format("VAB body does not fit SPU RAM"));
    }
    spu.upload(0, body);
    spu.set_main_volume(scale_7bit(PRIMER_MASTER_VOLUME), scale_7bit(PRIMER_MASTER_VOLUME));
    // The work area is the top of SPU RAM; the staged body has to end before it starts.
    let unit = ADDRESS_UNIT as usize;
    let body_end = body.len().div_ceil(unit) * unit;
    let base = REVERB_AREA_START.max(body_end as u32);
    let depth = scale_7bit(PRIMER_REVERB_DEPTH);
    spu.configure_reverb(&REVERB_PRESETS[PRESET_ROOM], base, depth, depth);
    Ok(())
}

/// The archive parts every sample or music item reads.
#[derive(Debug, Clone)]
pub(crate) struct Bank {
    pub(crate) body: Vec<u8>,
    pub(crate) tones: Vec<Tone>,
    pub(crate) vags: Vec<Vag>,
    pub(crate) master_volume: u8,
    pub(crate) master_pan: u8,
}

impl Bank {
    /// Bank number `bank` (0-based type-6 entry) of an archive.
    pub(crate) fn from_emi(emi: &Emi<'_>, bank: usize) -> Result<Self> {
        let header = emi.find(EntryType::Vh, bank)?;
        let body = emi.find(EntryType::Vb, bank)?;
        Self::from_parts(header, body)
    }

    pub(crate) fn from_parts(header: &[u8], body: &[u8]) -> Result<Self> {
        if body.len() > spu::RAM_SIZE {
            return Err(Error::format("VAB body does not fit SPU RAM"));
        }
        let vab = Vab::parse(header, body)?;
        Ok(Self {
            body: body.to_vec(),
            tones: vab.usable_tones().copied().collect(),
            vags: vab.vags,
            master_volume: vab.master_vol,
            master_pan: vab.master_pan,
        })
    }

    fn setup(&self, vag: &Vag, tone: &Tone, pitch: u16, left: u16, right: u16) -> VoiceSetup {
        VoiceSetup {
            start: u32::from((vag.offset >> 3) as u16) * ADDRESS_UNIT,
            repeat: u32::from((vag.repeat_offset() >> 3) as u16) * ADDRESS_UNIT,
            pitch,
            adsr1: tone.adsr1,
            adsr2: tone.adsr2,
            volume_left: left,
            volume_right: right,
            reverb_send: tone.reverb_send(),
        }
    }
}

/// One sequence event resolved onto the frame timeline.
#[derive(Debug, Clone, Copy)]
struct NoteEvent {
    kind: NoteKind,
    channel: u8,
    note: u8,
    velocity: u8,
    program: u8,
    volume: u8,
    pan: u8,
    bend: u8,
    frame: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoteKind {
    Bend,
    On,
    Off,
    Volume,
    Pan,
}

#[derive(Debug, Clone, Copy)]
struct ChannelState {
    program: u8,
    volume: u8,
    pan: u8,
    bend: u8,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self { program: 0, volume: MIX_MAX, pan: MIX_CENTER, bend: MIX_CENTER }
    }
}

/// The sequence's loop window from its CC 99 markers.
#[derive(Debug, Clone, Copy)]
struct LoopWindow {
    start_frame: i64,
    end_frame: i64,
    start_event: usize,
    end_event: usize,
}

impl LoopWindow {
    const NONE: Self = Self { start_frame: -1, end_frame: -1, start_event: 0, end_event: 0 };

    fn exists(&self) -> bool {
        self.start_frame >= 0 && self.end_frame > self.start_frame
    }
}

struct Timeline {
    events: Vec<NoteEvent>,
    window: LoopWindow,
    end_frame: i64,
}

fn build_timeline(sequence: &Sequence) -> Timeline {
    let resolution = clock::effective_resolution(sequence);
    let mut step = clock::driver_tick_step(sequence.tempo_us, resolution);
    let mut units = 0i64;
    let mut channels = [ChannelState::default(); crate::format::sep::CHANNELS];
    let mut events: Vec<NoteEvent> = Vec::new();
    let mut window = LoopWindow::NONE;
    let mut end_frame = 0;

    for event in &sequence.events {
        units += clock::DRIVER_UNITS_PER_TICK * i64::from(event.delta);
        let frame = (clock::driver_tick_of(units, step) + 1) * clock::DRIVER_FRAMES_PER_TICK;
        end_frame = frame;
        if event.kind() == Kind::Meta {
            if let Some(tempo) = event.tempo() {
                step = clock::driver_tick_step(tempo, resolution);
            }
            continue;
        }
        let state = &mut channels[usize::from(event.channel())];
        let base = NoteEvent {
            kind: NoteKind::Off,
            channel: event.channel(),
            note: 0,
            velocity: 0,
            program: 0,
            volume: 0,
            pan: MIX_CENTER,
            bend: MIX_CENTER,
            frame,
        };
        match event.kind() {
            Kind::Control => {
                if event.data1 == clock::CC_LOOP {
                    match event.data2 {
                        clock::LOOP_START => {
                            window.start_frame = frame;
                            window.start_event = events.len();
                        }
                        clock::LOOP_END => {
                            window.end_frame = frame;
                            window.end_event = events.len();
                        }
                        _ => {}
                    }
                } else if event.data1 == clock::CC_VOLUME || event.data1 == clock::CC_PAN {
                    let kind = if event.data1 == clock::CC_VOLUME {
                        state.volume = event.data2;
                        NoteKind::Volume
                    } else {
                        state.pan = event.data2;
                        NoteKind::Pan
                    };
                    events.push(NoteEvent { kind, volume: state.volume, pan: state.pan, ..base });
                }
            }
            Kind::Program => state.program = event.data1,
            Kind::PitchBend => {
                state.bend = event.data2;
                events.push(NoteEvent { kind: NoteKind::Bend, bend: state.bend, ..base });
            }
            Kind::NoteOn if event.data2 != 0 => events.push(NoteEvent {
                kind: NoteKind::On,
                note: event.data1,
                velocity: event.data2,
                program: state.program,
                volume: state.volume,
                pan: state.pan,
                bend: state.bend,
                ..base
            }),
            Kind::NoteOn | Kind::NoteOff => events.push(NoteEvent { kind: NoteKind::Off, note: event.data1, ..base }),
            Kind::PolyAftertouch | Kind::ChannelAftertouch | Kind::Meta => {}
        }
    }
    Timeline { events, window, end_frame }
}

#[derive(Debug, Clone, Copy)]
struct VoiceState {
    active: bool,
    channel: u8,
    note: u8,
    center_note: u8,
    center_shift: u8,
    bend: u8,
    bend_min: u8,
    bend_max: u8,
    priority: u8,
    generation: u64,
    started_frame: i64,
    tone: usize,
    mix: NoteEvent,
}

/// A sequence played through the SPU: one pass, loop control, seek.
pub(crate) struct MusicEngine {
    bank: Bank,
    events: Vec<NoteEvent>,
    window: LoopWindow,
    spu: Spu,
    model: ReverbModel,
    voices: [Option<VoiceState>; VOICE_COUNT],
    frame: i64,
    total_frames: i64,
    next_event: usize,
    next_generation: u64,
    loop_mode: Loop,
    loops_left: u32,
    scratch: Vec<i16>,
}

impl MusicEngine {
    /// Prepare sequence `sequence` (index into the SEP file) of an archive.
    pub(crate) fn new(archive: &[u8], sequence: u32, model: ReverbModel) -> Result<Self> {
        let emi = Emi::parse(archive)?;
        let header = emi.find(EntryType::Vh, 0)?;
        let body = emi.find(EntryType::Vb, 0)?;
        let seq_bytes = emi.find(EntryType::Seq, 0)?;
        let sep = Sep::parse(seq_bytes)?;
        let selected = sep
            .sequences
            .get(sequence as usize)
            .ok_or_else(|| Error::invalid(format!("sequence {sequence} is out of range for this archive")))?;
        let bank = Bank::from_parts(header, body)?;
        if !selected.has_notes() {
            return Err(Error::format(format!("sequence {sequence} of this archive has no events")));
        }
        let used = selected.programs_used();
        let available = bank.tones.iter().fold(0u128, |m, t| m | 1u128 << (t.program & 0x7F));
        if used & available == 0 {
            return Err(Error::format(format!(
                "sequence {sequence} asks for program {}, which this bank has no tone for",
                crate::format::sep::lowest_program(used)
            )));
        }
        let timeline = build_timeline(selected);
        let mut engine = Self {
            bank,
            events: timeline.events,
            window: timeline.window,
            spu: Spu::with_reverb_model(model),
            model,
            voices: [None; VOICE_COUNT],
            frame: 0,
            total_frames: timeline.end_frame + TAIL_SECONDS * i64::from(VAB_SAMPLE_RATE),
            next_event: 0,
            next_generation: 1,
            loop_mode: Loop::Off,
            loops_left: 0,
            scratch: alloc::vec![0i16; SEEK_CHUNK_FRAMES * CHANNELS as usize],
        };
        configure_spu(&mut engine.spu, &engine.bank.body)?;
        Ok(engine)
    }

    pub(crate) fn duration(&self) -> f64 {
        self.total_frames as f64 / f64::from(VAB_SAMPLE_RATE)
    }

    pub(crate) fn position(&self) -> f64 {
        self.frame as f64 / f64::from(VAB_SAMPLE_RATE)
    }

    pub(crate) fn loop_window(&self) -> Option<(f64, f64)> {
        self.window.exists().then(|| {
            (
                self.window.start_frame as f64 / f64::from(VAB_SAMPLE_RATE),
                self.window.end_frame as f64 / f64::from(VAB_SAMPLE_RATE),
            )
        })
    }

    pub(crate) fn set_loop(&mut self, mode: Loop) {
        self.loop_mode = mode;
        self.loops_left = if let Loop::Times(n) = mode { n } else { 0 };
    }

    fn wraps(&self) -> bool {
        self.loop_mode == Loop::Forever || self.loops_left > 0
    }

    fn loop_begin(&self) -> i64 {
        if self.window.exists() { self.window.start_frame } else { 0 }
    }

    fn loop_end(&self) -> i64 {
        if self.window.exists() { self.window.end_frame } else { self.total_frames }
    }

    fn rewind(&mut self) -> Result<()> {
        self.spu = Spu::with_reverb_model(self.model);
        self.voices = [None; VOICE_COUNT];
        self.next_generation = 1;
        self.next_event = 0;
        self.frame = 0;
        self.loops_left = if let Loop::Times(n) = self.loop_mode { n } else { 0 };
        configure_spu(&mut self.spu, &self.bank.body)
    }

    /// Fill `out` (interleaved stereo); returns frames written, 0 at the end.
    pub(crate) fn render(&mut self, out: &mut [i16]) -> Result<usize> {
        let requested = out.len() / CHANNELS as usize;
        let mut written = 0;
        while written < requested {
            if self.wraps() && self.frame >= self.loop_end() {
                self.jump_to_loop_start();
                if self.loop_mode != Loop::Forever {
                    self.loops_left -= 1;
                }
                continue;
            }
            let mut chunk = requested - written;
            if self.wraps() {
                let until_wrap = self.loop_end() - self.frame;
                if until_wrap > 0 && (until_wrap as usize) < chunk {
                    chunk = until_wrap as usize;
                }
            }
            let slice = &mut out[written * 2..(written + chunk) * 2];
            let filled = self.pump(slice);
            if filled == 0 {
                break;
            }
            written += filled;
        }
        Ok(written)
    }

    /// The driver's loop is a jump of its tick clock, not a restart: only the
    /// event cursor moves, the SPU keeps playing the voices sounding at the seam.
    fn jump_to_loop_start(&mut self) {
        if self.window.exists() {
            let end = self.window.end_event;
            self.apply_events(end);
        }
        self.frame = self.loop_begin();
        self.next_event = if self.window.exists() { self.window.start_event } else { 0 };
    }

    seek_body!(rewind);

    fn pump(&mut self, out: &mut [i16]) -> usize {
        let requested = out.len() / 2;
        let remaining = self.total_frames - self.frame;
        let frames = if remaining > 0 { requested.min(remaining as usize) } else { 0 };
        let mut written = 0;
        while written < frames {
            self.apply_events(usize::MAX);
            // Render up to the next event so a control change lands on its frame.
            let boundary =
                self.events.get(self.next_event).map_or(self.total_frames, |e| e.frame).min(self.total_frames);
            let chunk = (boundary - self.frame).max(1).min((frames - written) as i64) as usize;
            self.spu.render(&mut out[written * 2..(written + chunk) * 2]);
            self.frame += chunk as i64;
            written += chunk;
        }
        written
    }

    fn apply_events(&mut self, limit: usize) {
        let end = limit.min(self.events.len());
        while self.next_event < end && self.events[self.next_event].frame <= self.frame {
            let event = self.events[self.next_event];
            self.next_event += 1;
            match event.kind {
                NoteKind::Bend => self.apply_bend(&event),
                NoteKind::On => self.begin_note(&event),
                NoteKind::Off => self.end_note(&event),
                NoteKind::Volume | NoteKind::Pan => self.apply_mix(&event),
            }
        }
    }

    fn voice_pitch(&self, voice: &VoiceState) -> u16 {
        let bend = i32::from(voice.bend) - i32::from(MIX_CENTER);
        let mut note = i32::from(voice.note);
        let mut fine = 0;
        if bend < 0 {
            let scaled = bend * i32::from(voice.bend_min);
            note += scaled / BEND_DOWN_STEPS - 1;
            fine = FINE_PER_STEP * (scaled % BEND_DOWN_STEPS) + FINE_PER_SEMITONE - 1;
        } else if bend > 0 {
            let scaled = bend * i32::from(voice.bend_max);
            note += scaled / BEND_UP_STEPS;
            fine = FINE_PER_STEP * (scaled % BEND_UP_STEPS);
        }
        pitch_from_note(note, fine, i32::from(voice.center_note), i32::from(voice.center_shift))
    }

    fn apply_bend(&mut self, event: &NoteEvent) {
        for slot in 0..VOICE_COUNT {
            let Some(mut voice) = self.voices[slot] else { continue };
            if !voice.active || voice.channel != event.channel {
                continue;
            }
            voice.bend = event.bend;
            self.spu.set_voice_pitch(slot, self.voice_pitch(&voice));
            self.voices[slot] = Some(voice);
        }
    }

    fn end_note(&mut self, event: &NoteEvent) {
        for slot in 0..VOICE_COUNT {
            if let Some(voice) = &self.voices[slot]
                && voice.active
                && voice.channel == event.channel
                && voice.note == event.note
            {
                self.spu.key_off(slot);
            }
        }
    }

    fn levels(&self, tone: &Tone, event: &NoteEvent) -> (f32, f32, f32) {
        let scale = f32::from(MIX_MAX);
        let volume = tone_volume(self.bank.master_volume, tone)
            * (f32::from(event.volume) / scale)
            * (f32::from(event.velocity) / scale);
        let pan = (tone_pan(self.bank.master_pan, tone) + (i32::from(event.pan) - i32::from(MIX_CENTER)))
            .clamp(0, i32::from(MIX_MAX));
        let (left, right) = pan_gains(pan);
        (volume, left, right)
    }

    fn apply_mix(&mut self, event: &NoteEvent) {
        for slot in 0..VOICE_COUNT {
            let Some(mut voice) = self.voices[slot] else { continue };
            if !voice.active || voice.channel != event.channel {
                continue;
            }
            if event.kind == NoteKind::Volume {
                voice.mix.volume = event.volume;
            } else {
                voice.mix.pan = event.pan;
            }
            let (volume, left, right) = self.levels(&self.bank.tones[voice.tone], &voice.mix);
            self.spu.set_voice_volume(slot, fixed_voice_volume(volume * left), fixed_voice_volume(volume * right));
            self.voices[slot] = Some(voice);
        }
    }

    fn begin_note(&mut self, event: &NoteEvent) {
        let program_tones = self.bank.tones.iter().filter(|t| t.program == i16::from(event.program)).count();
        if program_tones == 0 {
            return;
        }
        let covering: Vec<usize> = self
            .bank
            .tones
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.program == i16::from(event.program) && event.note >= t.min_note && event.note <= t.max_note
            })
            .filter(|(_, t)| t.vag != 0 && (t.vag as usize) < self.bank.vags.len())
            .map(|(i, _)| i)
            .collect();
        for index in covering {
            let priority = self.bank.tones[index].priority;
            if let Some(slot) = self.pick_voice(priority) {
                self.start_voice(slot, index, event);
            }
        }
    }

    /// First free slot (dropping the ones the SPU has finished), else a steal victim.
    fn pick_voice(&mut self, priority: u8) -> Option<usize> {
        for slot in 0..VOICE_COUNT {
            if let Some(voice) = &mut self.voices[slot] {
                if voice.active && !self.spu.voice_active(slot) {
                    voice.active = false;
                }
                if voice.active {
                    continue;
                }
            }
            return Some(slot);
        }
        self.steal_voice(priority)
    }

    /// Lowest priority, then quietest envelope, then oldest.
    fn steal_voice(&self, priority: u8) -> Option<usize> {
        let mut victim = None;
        let (mut lowest, mut smallest, mut oldest) = (priority, 0xFFFFu16, u64::MAX);
        for (slot, voice) in self.voices.iter().enumerate() {
            let Some(voice) = voice else { continue };
            // A freshly keyed voice has a zero attack envelope; it must survive the
            // other note-ons scheduled in this frame.
            if voice.started_frame == self.frame {
                continue;
            }
            let level = self.spu.envelope_level(slot);
            if voice.priority < lowest
                || (voice.priority == lowest && level < smallest)
                || (voice.priority == lowest && level == smallest && voice.generation < oldest)
            {
                lowest = voice.priority;
                smallest = level;
                oldest = voice.generation;
                victim = Some(slot);
            }
        }
        victim
    }

    fn start_voice(&mut self, slot: usize, tone_index: usize, event: &NoteEvent) {
        let tone = self.bank.tones[tone_index];
        let vag = self.bank.vags[tone.vag as usize];
        let (volume, left, right) = self.levels(&tone, event);
        let voice = VoiceState {
            active: true,
            channel: event.channel,
            note: event.note,
            center_note: tone.center_note,
            center_shift: tone.shift,
            bend: MIX_CENTER,
            bend_min: tone.pitch_bend_min,
            bend_max: tone.pitch_bend_max,
            priority: tone.priority,
            generation: self.next_generation,
            started_frame: self.frame,
            tone: tone_index,
            mix: *event,
        };
        self.next_generation += 1;
        let pitch = self.voice_pitch(&voice);
        let setup =
            self.bank.setup(&vag, &tone, pitch, fixed_voice_volume(volume * left), fixed_voice_volume(volume * right));
        self.spu.key_on(slot, &setup);
        self.voices[slot] = Some(voice);
    }
}

/// One VAG played as a `sample:` item.
pub(crate) struct SampleEngine {
    bank: Bank,
    vag_index: u32,
    spu: Spu,
    model: ReverbModel,
    pass_frames: i64,
    frame: i64,
    total_frames: i64,
    scratch: Vec<i16>,
}

impl SampleEngine {
    pub(crate) fn new(archive: &[u8], bank: usize, vag_index: u32, model: ReverbModel) -> Result<Self> {
        let emi = Emi::parse(archive)?;
        let bank = Bank::from_emi(&emi, bank)?;
        let vag = *bank
            .vags
            .get(vag_index as usize)
            .filter(|_| vag_index != 0)
            .ok_or_else(|| Error::invalid(format!("VAG {vag_index} is out of range for this bank")))?;
        if vag.is_empty() {
            return Err(Error::format("VAG is empty"));
        }
        let mut engine = Self {
            bank,
            vag_index,
            spu: Spu::with_reverb_model(model),
            model,
            pass_frames: 0,
            frame: 0,
            total_frames: 0,
            scratch: alloc::vec![0i16; SEEK_CHUNK_FRAMES * 2],
        };
        engine.start()?;
        engine.total_frames = if vag.has_end && !vag.has_loop {
            engine.pass_frames
        } else {
            engine.pass_frames + TAIL_SECONDS * i64::from(VAB_SAMPLE_RATE)
        };
        Ok(engine)
    }

    /// Fresh device, primer, sample keyed on at frame 0.
    fn start(&mut self) -> Result<()> {
        self.spu = Spu::with_reverb_model(self.model);
        self.frame = 0;
        configure_spu(&mut self.spu, &self.bank.body)?;
        let vag = self.bank.vags[self.vag_index as usize];
        // The tone that names this sample supplies envelope, volume and pitch.
        let named =
            self.bank.tones.iter().find(|t| {
                t.vag == self.vag_index as i16 && t.vol != 0 && t.program_vol != 0 && t.min_note <= t.max_note
            });
        let (adsr1, adsr2, reverb, volume, pitch) = match named {
            Some(tone) => {
                let note = i32::from(tone.center_note.clamp(tone.min_note, tone.max_note));
                (
                    tone.adsr1,
                    tone.adsr2,
                    tone.reverb_send(),
                    tone_volume(self.bank.master_volume, tone),
                    pitch_from_note(note, 0, i32::from(tone.center_note), i32::from(tone.shift)),
                )
            }
            None => (DEFAULT_ADSR1, DEFAULT_ADSR2, false, 1.0, PITCH_ONE as u16),
        };
        if pitch == 0 {
            return Err(Error::format("sample pitch is zero"));
        }
        // Audition placement is centred: both sides get the full voice.
        let register = fixed_voice_volume(volume);
        self.pass_frames = (i64::from(vag.frames) * i64::from(PITCH_ONE) + i64::from(pitch) - 1) / i64::from(pitch);
        let setup = VoiceSetup {
            start: u32::from((vag.offset >> 3) as u16) * ADDRESS_UNIT,
            repeat: u32::from((vag.repeat_offset() >> 3) as u16) * ADDRESS_UNIT,
            pitch,
            adsr1,
            adsr2,
            volume_left: register,
            volume_right: register,
            reverb_send: reverb,
        };
        self.spu.key_on(0, &setup);
        Ok(())
    }

    pub(crate) fn duration(&self) -> f64 {
        self.total_frames as f64 / f64::from(VAB_SAMPLE_RATE)
    }

    pub(crate) fn position(&self) -> f64 {
        self.frame as f64 / f64::from(VAB_SAMPLE_RATE)
    }

    pub(crate) fn render(&mut self, out: &mut [i16]) -> Result<usize> {
        Ok(self.pump(out))
    }

    fn pump(&mut self, out: &mut [i16]) -> usize {
        let remaining = self.total_frames - self.frame;
        if remaining <= 0 {
            return 0;
        }
        let mut frames = (out.len() / 2).min(remaining as usize);
        if self.frame < self.pass_frames {
            frames = frames.min((self.pass_frames - self.frame) as usize);
        }
        if self.frame == self.pass_frames && self.total_frames > self.pass_frames {
            self.spu.key_off(0);
        }
        if frames == 0 {
            return 0;
        }
        self.spu.render(&mut out[..frames * 2]);
        self.frame += frames as i64;
        frames
    }

    seek_body!(start);
}
