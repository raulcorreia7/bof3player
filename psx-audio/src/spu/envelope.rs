// SPDX-License-Identifier: MIT
//! ADSR envelope generator (psx-spx, "SPU Volume and ADSR Generator").
//!
//! The 32-bit ADSR word is split over two 16-bit registers, `adsr1` (low) and
//! `adsr2` (high). The generator runs once per 44.1 kHz output sample.

/// Highest envelope level (`+7FFFh`).
pub(crate) const LEVEL_MAX: i32 = 0x7FFF;
/// Attack becomes exponential above this level (psx-spx: "AdsrLevel>6000h").
const EXPONENTIAL_ATTACK_KNEE: i32 = 0x6000;
/// Sustain level is `(N + 1) * 800h`.
const SUSTAIN_LEVEL_UNIT: i32 = 0x800;
/// Shift values below this change the step; at or above it they change the wait.
const RATE_PIVOT: i32 = 11;
/// Fractional bits of the exponential-decrease scaling (`AdsrLevel / 8000h`).
const LEVEL_FRACTION_BITS: u32 = 15;
/// The wait is multiplied by four for exponential increase above the knee.
const EXPONENTIAL_SLOWDOWN: i64 = 4;
/// Longest wait, as a power of two, kept clear of overflow.
const MAX_WAIT_SHIFT: i32 = 40;

/// Which part of the envelope a voice is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Rising to full level.
    Attack,
    /// Falling to the sustain level.
    Decay,
    /// Holding (or slowly moving) until key off.
    Sustain,
    /// Falling to zero after key off.
    Release,
    /// Silent; the voice is finished.
    Off,
}

/// The running envelope of one voice.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Envelope {
    /// Current level, `0..=0x7FFF`.
    pub(crate) level: i32,
    /// Current phase.
    pub(crate) phase: Phase,
    /// Ticks left before the next step.
    wait: i64,
    adsr1: u16,
    adsr2: u16,
}

/// One phase's rate parameters, decoded from the ADSR registers.
struct Rate {
    exponential: bool,
    decrease: bool,
    shift: i32,
    step: i32,
}

impl Envelope {
    /// An envelope that is off.
    pub(crate) const fn off() -> Self {
        Self { level: 0, phase: Phase::Off, wait: 0, adsr1: 0, adsr2: 0 }
    }

    /// Key on: level zero, attack phase, with the given ADSR registers.
    pub(crate) fn start(adsr1: u16, adsr2: u16) -> Self {
        let mut envelope = Self { level: 0, phase: Phase::Attack, wait: 0, adsr1, adsr2 };
        envelope.reload();
        envelope
    }

    /// Key off: jump to release (no effect on a finished voice).
    pub(crate) fn release(&mut self) {
        if self.phase != Phase::Off && self.phase != Phase::Release {
            self.enter(Phase::Release);
        }
    }

    fn rate(&self) -> Rate {
        let a1 = i32::from(self.adsr1);
        let a2 = i32::from(self.adsr2);
        match self.phase {
            Phase::Attack => Rate {
                exponential: a1 & 0x8000 != 0,
                decrease: false,
                shift: (a1 >> 10) & 0x1F,
                step: 7 - ((a1 >> 8) & 3),
            },
            Phase::Decay => Rate { exponential: true, decrease: true, shift: (a1 >> 4) & 0x0F, step: -8 },
            Phase::Sustain => {
                let decrease = a2 & 0x4000 != 0;
                let bits = (a2 >> 6) & 3;
                Rate {
                    exponential: a2 & 0x8000 != 0,
                    decrease,
                    shift: (a2 >> 8) & 0x1F,
                    step: if decrease { -8 + bits } else { 7 - bits },
                }
            }
            Phase::Release | Phase::Off => {
                Rate { exponential: a2 & 0x20 != 0, decrease: true, shift: a2 & 0x1F, step: -8 }
            }
        }
    }

    /// Ticks one step waits (`AdsrCycles`) and the step itself (`AdsrStep`).
    fn cycles_and_step(&self) -> (i64, i32) {
        let rate = self.rate();
        let mut cycles = 1i64 << (rate.shift - RATE_PIVOT).clamp(0, MAX_WAIT_SHIFT);
        let mut step = rate.step << (RATE_PIVOT - rate.shift).max(0);
        if rate.exponential && !rate.decrease && self.level > EXPONENTIAL_ATTACK_KNEE {
            cycles *= EXPONENTIAL_SLOWDOWN;
        }
        if rate.exponential && rate.decrease {
            step = (step * self.level) >> LEVEL_FRACTION_BITS;
        }
        (cycles, step)
    }

    fn reload(&mut self) {
        self.wait = self.cycles_and_step().0;
    }

    /// Advance one output sample.
    pub(crate) fn tick(&mut self) {
        if self.phase == Phase::Off {
            return;
        }
        self.wait -= 1;
        if self.wait > 0 {
            return;
        }
        let (_, step) = self.cycles_and_step();
        self.level = (self.level + step).clamp(0, LEVEL_MAX);
        let before = self.phase;
        self.advance_phase();
        if self.phase == before {
            // The knee and the exponential scaling depend on the new level.
            self.reload();
        }
    }

    fn advance_phase(&mut self) {
        match self.phase {
            Phase::Attack if self.level >= LEVEL_MAX => self.enter(Phase::Decay),
            Phase::Decay if self.level <= (i32::from(self.adsr1 & 0x0F) + 1) * SUSTAIN_LEVEL_UNIT => {
                self.enter(Phase::Sustain);
            }
            Phase::Release if self.level == 0 => self.enter(Phase::Off),
            _ => {}
        }
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        if phase != Phase::Off {
            self.reload();
        }
    }

    /// Runs whole steps, as [`Envelope::tick`] does, until `done` holds. Returns the output samples those
    /// steps took, or `None` when the level stops moving or the phase does not end within [`MAX_STEPS`].
    fn run_until(&mut self, done: impl Fn(&Envelope) -> bool) -> Option<u64> {
        let mut samples = 0u64;
        for _ in 0..MAX_STEPS {
            if done(self) {
                return Some(samples);
            }
            let (cycles, step) = self.cycles_and_step();
            if step == 0 {
                return None;
            }
            samples += cycles as u64;
            self.level = (self.level + step).clamp(0, LEVEL_MAX);
            self.advance_phase();
        }
        None
    }
}

/// How long each phase of a voice's envelope takes, in output samples at 44.1 kHz. Phases that
/// depend on the level, such as an exponential decay, are timed step by step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timing {
    /// From key on until the level reaches full scale.
    pub(crate) attack: u64,
    /// From full scale until the level reaches the sustain level.
    pub(crate) decay: u64,
    /// The sustain level, `(N + 1) * 800h`, from the decay register. Sustain holds this level; a sustain
    /// that decreases after its start is not part of the timing.
    pub(crate) sustain_level: i32,
    /// From the level the decay ends at until the level reaches zero after key off.
    pub(crate) release: u64,
}

/// The phase timing of an envelope with the given ADSR registers, or `None` when a phase does not end.
/// The steps are those of [`Envelope::tick`]: one step after each wait, with the wait of the level the
/// step leaves. The timing is computed step by step, so it is fast even for the slowest rates.
pub(crate) fn timing_of(adsr1: u16, adsr2: u16) -> Option<Timing> {
    let mut envelope = Envelope::start(adsr1, adsr2);
    let attack = envelope.run_until(|envelope| envelope.phase != Phase::Attack)?;
    let decay = envelope.run_until(|envelope| envelope.phase != Phase::Decay)?;
    let sustain_level = (i32::from(adsr1 & 0x0F) + 1) * SUSTAIN_LEVEL_UNIT;
    // Key off starts the release at the level the decay ended on, as `Envelope::release` does.
    let mut release = Envelope { level: envelope.level, phase: Phase::Release, wait: 0, adsr1, adsr2 };
    release.reload();
    let release = release.run_until(|envelope| envelope.phase == Phase::Off)?;
    Some(Timing { attack, decay, sustain_level, release })
}

/// The longest run of steps a phase may take: one step per level for a full sweep, with room to spare.
const MAX_STEPS: u32 = 1 << 16;
