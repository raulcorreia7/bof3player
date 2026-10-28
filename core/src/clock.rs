// SPDX-License-Identifier: MIT
//! The game's sequencer clock: how SEP delta times map to frames of the 44.1 kHz driver.
//!
//! The format (events, running status, MIDI export) is `psx_formats::sep`; this module only
//! knows how the retail driver counts time.

use psx_formats::VAB_SAMPLE_RATE;
use psx_formats::sep::Sequence;

/// Tempo used when a sequence states none: 120 BPM.
const DEFAULT_TEMPO_US: u32 = 500_000;
/// Resolution used when a sequence states none.
pub(crate) const DEFAULT_RESOLUTION: u32 = 48;
const MICROSECONDS_PER_MINUTE: i64 = 60_000_000;

/// Frames one driver tick covers: 44.1 kHz / 60.
pub(crate) const DRIVER_FRAMES_PER_TICK: i64 = 735;
/// Units one authored delta advances the sequence by. The driver's tick step and
/// its unit counter share this scale.
pub(crate) const DRIVER_UNITS_PER_TICK: i64 = 10;
/// Seconds of driver ticks per second: one tick is 1/60 s.
const TICKS_PER_BPM_UNIT: i64 = 3600;

/// Control change: volume.
pub(crate) const CC_VOLUME: u8 = 7;
/// Control change: pan.
pub(crate) const CC_PAN: u8 = 10;
/// Control change (NRPN MSB) that carries loop markers: 20 starts the loop, 30 ends it.
pub(crate) const CC_LOOP: u8 = 99;
/// Loop marker value: loop start.
pub(crate) const LOOP_START: u8 = 20;
/// Loop marker value: loop end.
pub(crate) const LOOP_END: u8 = 30;

fn div_round_half_up(value: i64, divisor: i64) -> i64 {
    let quotient = value / divisor;
    if value % divisor > divisor / 2 { quotient + 1 } else { quotient }
}

/// Units one driver tick advances a sequence by at `tempo_us` and `resolution`.
#[must_use]
pub(crate) fn driver_tick_step(tempo_us: u32, resolution: u32) -> i64 {
    let tempo = if tempo_us > 0 { i64::from(tempo_us) } else { i64::from(DEFAULT_TEMPO_US) };
    let bpm = div_round_half_up(MICROSECONDS_PER_MINUTE, tempo);
    let step = div_round_half_up(DRIVER_UNITS_PER_TICK * i64::from(resolution) * bpm, TICKS_PER_BPM_UNIT);
    step.max(1)
}

/// The driver tick a running unit count falls in.
#[must_use]
pub(crate) fn driver_tick_of(units: i64, step: i64) -> i64 {
    ((units + step - 1) / step - 1).max(0)
}

/// A sequence's resolution with the default applied.
#[must_use]
pub(crate) fn effective_resolution(sequence: &Sequence) -> u32 {
    if sequence.resolution > 0 { u32::from(sequence.resolution) } else { DEFAULT_RESOLUTION }
}

/// The frame the driver's pass ends on (its last event on the driver clock).
#[must_use]
pub(crate) fn driver_frames(sequence: &Sequence) -> u64 {
    if sequence.events.is_empty() {
        return 0;
    }
    let resolution = effective_resolution(sequence);
    let mut step = driver_tick_step(sequence.tempo_us, resolution);
    let mut units = 0i64;
    let mut frame = 0i64;
    for event in &sequence.events {
        units += DRIVER_UNITS_PER_TICK * i64::from(event.delta);
        if let Some(tempo) = event.tempo() {
            step = driver_tick_step(tempo, resolution);
        }
        frame = (driver_tick_of(units, step) + 1) * DRIVER_FRAMES_PER_TICK;
    }
    frame as u64
}

/// Length of a sequence on the driver clock, in seconds.
#[must_use]
pub(crate) fn seconds(sequence: &Sequence) -> f64 {
    driver_frames(sequence) as f64 / f64::from(VAB_SAMPLE_RATE)
}
