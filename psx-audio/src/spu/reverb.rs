// SPDX-License-Identifier: MIT
//! SPU reverb unit (psx-spx, "SPU Reverb Registers" and "SPU Reverb Formula").
//!
//! The delay lines live in SPU RAM between `base` and the end of RAM. One
//! output side is computed per 44.1 kHz sample: left on even samples, right on
//! odd ones, the buffer address advancing after the right side. Address
//! registers count 8-byte units (psx-spx: "addresses in SPU memory (divided by 8)").

use super::RAM_SIZE;
use super::tables::ReverbPreset;

/// Reverb addresses are stored in units of 8 bytes.
const ADDRESS_UNIT: u32 = 8;
/// Last byte pair of SPU RAM (psx-spx: "7FFFEh").
const RAM_LAST_PAIR: u32 = RAM_SIZE as u32 - 2;
/// Bytes per reverb buffer sample.
const SAMPLE_BYTES: u32 = 2;
/// Volume registers are signed 15-bit fractions of `8000h`.
const VOLUME_BITS: u32 = 15;

/// Which delay-line timing the unit uses.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_audio::spu::ReverbModel::default(), psx_audio::spu::ReverbModel::Hardware);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReverbModel {
    /// Hardware timing from psx-spx: address registers count 8-byte units and
    /// the buffer address advances once per 22.05 kHz cycle.
    #[default]
    Hardware,
    /// The timing of the earlier renderer: address registers count 2-byte
    /// samples and the buffer advances once per 44.1 kHz sample, with both
    /// sides computed together. Delays come out 8 times shorter than on
    /// hardware. Kept only so old renders can be reproduced.
    Legacy,
}

impl ReverbModel {
    /// Bytes one address-register step covers.
    const fn unit(self) -> u32 {
        match self {
            Self::Hardware => ADDRESS_UNIT,
            Self::Legacy => SAMPLE_BYTES,
        }
    }
}

/// Unit state: registers, buffer address and the per-sample inputs.
#[derive(Debug, Clone)]
pub(crate) struct Reverb {
    regs: [u16; 32],
    base: u32,
    cursor: u32,
    /// Output volume left, as a signed register value.
    pub(crate) out_left: i16,
    /// Output volume right.
    pub(crate) out_right: i16,
    /// Master write enable (SPUCNT bit 7).
    pub(crate) enabled: bool,
    tick: u32,
    out: (i32, i32),
    model: ReverbModel,
}

// Register indices, in psx-spx order (rev00..rev1F).
const D_APF1: usize = 0;
const D_APF2: usize = 1;
const V_IIR: usize = 2;
const V_COMB1: usize = 3;
const V_COMB2: usize = 4;
const V_COMB3: usize = 5;
const V_COMB4: usize = 6;
const V_WALL: usize = 7;
const V_APF1: usize = 8;
const V_APF2: usize = 9;
const M_LSAME: usize = 10;
const M_RSAME: usize = 11;
const M_LCOMB1: usize = 12;
const M_RCOMB1: usize = 13;
const M_LCOMB2: usize = 14;
const M_RCOMB2: usize = 15;
const D_LSAME: usize = 16;
const D_RSAME: usize = 17;
const M_LDIFF: usize = 18;
const M_RDIFF: usize = 19;
const M_LCOMB3: usize = 20;
const M_RCOMB3: usize = 21;
const M_LCOMB4: usize = 22;
const M_RCOMB4: usize = 23;
const D_LDIFF: usize = 24;
const D_RDIFF: usize = 25;
const M_LAPF1: usize = 26;
const M_RAPF1: usize = 27;
const M_LAPF2: usize = 28;
const M_RAPF2: usize = 29;
const V_LIN: usize = 30;
const V_RIN: usize = 31;

fn sat(value: i32) -> i32 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX))
}

fn mul(sample: i32, volume: u16) -> i32 {
    (sample * i32::from(volume as i16)) >> VOLUME_BITS
}

impl Reverb {
    /// A unit with all registers zero and the work area at `base` (bytes).
    pub(crate) fn new(base: u32) -> Self {
        Self {
            regs: [0; 32],
            base,
            cursor: base,
            out_left: 0,
            out_right: 0,
            enabled: false,
            tick: 0,
            out: (0, 0),
            model: ReverbModel::Hardware,
        }
    }

    /// Choose the delay-line timing; call before the first sample.
    pub(crate) fn set_model(&mut self, model: ReverbModel) {
        self.model = model;
    }

    /// Load the 32 registers of a preset.
    pub(crate) fn load_preset(&mut self, preset: &ReverbPreset) {
        self.regs = *preset;
    }

    /// Move the work area; this also resets the buffer address (psx-spx: writing
    /// mBASE sets the current buffer address).
    pub(crate) fn set_base(&mut self, base: u32) {
        self.base = base.min(RAM_LAST_PAIR);
        self.cursor = self.base;
    }

    fn wrap(&self, address: u32) -> u32 {
        let span = RAM_SIZE as u32 - self.base;
        let mut a = address & !1;
        if a >= RAM_SIZE as u32 || a < self.base {
            a = self.base + (a.wrapping_sub(self.base)) % span;
        }
        a
    }

    fn at(&self, register: usize, minus: u32) -> u32 {
        self.wrap(self.cursor.wrapping_add(u32::from(self.regs[register]) * self.model.unit()).wrapping_sub(minus))
    }

    fn read(&self, ram: &[u8], address: u32) -> i32 {
        let a = address as usize;
        i32::from(i16::from_le_bytes([ram[a], ram[a + 1]]))
    }

    fn write(&self, ram: &mut [u8], address: u32, value: i32) {
        if self.enabled {
            let a = address as usize;
            ram[a..a + 2].copy_from_slice(&(sat(value) as i16).to_le_bytes());
        }
    }

    /// Reflection stage: `[m] = (in + [d] * vWALL - [m-2]) * vIIR + [m-2]`.
    fn reflect(&self, ram: &mut [u8], input: i32, m: usize, d: usize) {
        let regs = &self.regs;
        let previous_at = self.at(m, SAMPLE_BYTES);
        let previous = self.read(ram, previous_at);
        let source = self.read(ram, self.at(d, 0));
        let value = mul(input + mul(source, regs[V_WALL]) - previous, regs[V_IIR]) + previous;
        self.write(ram, self.at(m, 0), value);
    }

    /// All-pass stage; returns the new running value.
    fn all_pass(&self, ram: &mut [u8], value: i32, m: usize, d: usize, volume: usize) -> i32 {
        let regs = &self.regs;
        let delayed_at = self
            .wrap(self.cursor.wrapping_add(u32::from(regs[m]).wrapping_sub(u32::from(regs[d])) * self.model.unit()));
        let delayed = self.read(ram, delayed_at);
        let value = value - mul(delayed, regs[volume]);
        self.write(ram, self.at(m, 0), value);
        mul(value, regs[volume]) + delayed
    }

    /// Comb filter bank over four taps.
    fn comb(&self, ram: &[u8], taps: [usize; 4]) -> i32 {
        let volumes = [V_COMB1, V_COMB2, V_COMB3, V_COMB4];
        let mut sum = 0;
        for (tap, volume) in taps.into_iter().zip(volumes) {
            sum += mul(self.read(ram, self.at(tap, 0)), self.regs[volume]);
        }
        sum
    }

    /// Run one 44.1 kHz sample: feed the reverb-send mix, return the reverb output.
    pub(crate) fn process(&mut self, ram: &mut [u8], input_left: i32, input_right: i32) -> (i32, i32) {
        let right_turn = self.tick & 1 == 1;
        self.tick = self.tick.wrapping_add(1);
        if self.model == ReverbModel::Legacy {
            let lin = mul(input_left, self.regs[V_LIN]);
            let rin = mul(input_right, self.regs[V_RIN]);
            self.reflect(ram, lin, M_LSAME, D_LSAME);
            self.reflect(ram, rin, M_RSAME, D_RSAME);
            self.reflect(ram, lin, M_LDIFF, D_RDIFF);
            self.reflect(ram, rin, M_RDIFF, D_LDIFF);
            let lo = self.comb(ram, [M_LCOMB1, M_LCOMB2, M_LCOMB3, M_LCOMB4]);
            let ro = self.comb(ram, [M_RCOMB1, M_RCOMB2, M_RCOMB3, M_RCOMB4]);
            let lo = self.all_pass(ram, lo, M_LAPF1, D_APF1, V_APF1);
            let ro = self.all_pass(ram, ro, M_RAPF1, D_APF1, V_APF1);
            let lo = self.all_pass(ram, lo, M_LAPF2, D_APF2, V_APF2);
            let ro = self.all_pass(ram, ro, M_RAPF2, D_APF2, V_APF2);
            self.out = (
                sat((lo * i32::from(self.out_left)) >> VOLUME_BITS),
                sat((ro * i32::from(self.out_right)) >> VOLUME_BITS),
            );
            self.cursor += SAMPLE_BYTES;
            if self.cursor > RAM_LAST_PAIR {
                self.cursor = self.base;
            }
            return self.out;
        }
        if right_turn {
            let rin = mul(input_right, self.regs[V_RIN]);
            self.reflect(ram, rin, M_RSAME, D_RSAME);
            self.reflect(ram, rin, M_RDIFF, D_LDIFF);
            let out = self.comb(ram, [M_RCOMB1, M_RCOMB2, M_RCOMB3, M_RCOMB4]);
            let out = self.all_pass(ram, out, M_RAPF1, D_APF1, V_APF1);
            let out = self.all_pass(ram, out, M_RAPF2, D_APF2, V_APF2);
            self.out.1 = sat((out * i32::from(self.out_right)) >> VOLUME_BITS);
            self.cursor += SAMPLE_BYTES;
            if self.cursor > RAM_LAST_PAIR {
                self.cursor = self.base;
            }
        } else {
            let lin = mul(input_left, self.regs[V_LIN]);
            self.reflect(ram, lin, M_LSAME, D_LSAME);
            self.reflect(ram, lin, M_LDIFF, D_RDIFF);
            let out = self.comb(ram, [M_LCOMB1, M_LCOMB2, M_LCOMB3, M_LCOMB4]);
            let out = self.all_pass(ram, out, M_LAPF1, D_APF1, V_APF1);
            let out = self.all_pass(ram, out, M_LAPF2, D_APF2, V_APF2);
            self.out.0 = sat((out * i32::from(self.out_left)) >> VOLUME_BITS);
        }
        self.out
    }
}
