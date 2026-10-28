//! PlayStation ADPCM: the 16-byte SPU block and the CD-ROM XA predictor.
//!
//! Sources: psx-spx "SPU ADPCM Samples" (block layout, flag bits, "same
//! algorithm as XA-ADPCM") and "CDROM XA Audio ADPCM Compression" (filter
//! tables, shift rule, `(old*f0 + older*f1 + 32) / 64`).
//!
//! # Examples
//!
//! ```
//! use psx_formats::adpcm;
//!
//! let pcm = vec![0i16; adpcm::BLOCK_SAMPLES];
//! let stream = adpcm::encode_stream(&pcm);
//! assert_eq!(psx_formats::adpcm::decode_stream(&stream), pcm);
//! ```

use alloc::vec::Vec;

/// Bytes in one SPU ADPCM block (psx-spx, SPU ADPCM Samples).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::adpcm::BLOCK_BYTES, 16);
/// ```
pub const BLOCK_BYTES: usize = 16;
/// Samples one block decodes to.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::adpcm::BLOCK_SAMPLES, 28);
/// ```
pub const BLOCK_SAMPLES: usize = 28;

/// Block flag bit 0: loop end (set ENDX, jump to the repeat address).
///
/// # Examples
///
/// ```
/// let flags = 0x23;
/// assert_ne!(flags & psx_formats::adpcm::FLAG_END, 0);
/// ```
pub const FLAG_END: u8 = 0x01;
/// Block flag bit 1: with `FLAG_END`, repeat instead of muting.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::adpcm::FLAG_END | psx_formats::adpcm::FLAG_REPEAT, 0x03);
/// ```
pub const FLAG_REPEAT: u8 = 0x02;
/// Block flag bit 2: loop start (copy the current address to the repeat address).
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::adpcm::FLAG_LOOP_START, 0x04);
/// ```
pub const FLAG_LOOP_START: u8 = 0x04;

/// Filter coefficients applied to the previous sample (psx-spx, XA-ADPCM `pos` table).
///
/// # Examples
///
/// ```
/// let filter = 1;
/// assert_eq!(psx_formats::adpcm::FILTER_POS[filter], 60);
/// ```
pub const FILTER_POS: [i32; 5] = [0, 60, 115, 98, 122];
/// Filter coefficients applied to the sample before that (psx-spx, XA-ADPCM `neg` table).
///
/// # Examples
///
/// ```
/// let filter = 4;
/// assert_eq!(psx_formats::adpcm::FILTER_NEG[filter], -60);
/// ```
pub const FILTER_NEG: [i32; 5] = [0, 0, -52, -55, -60];

/// Highest shift value that is not reserved; 13..=15 act as [`RESERVED_SHIFT`].
const MAX_SHIFT: u8 = 12;
/// Shift used for the reserved values 13..=15 (psx-spx: "Same as 9").
const RESERVED_SHIFT: u8 = 9;
/// Fractional bits of the filter coefficients: they are 1/64 steps.
const COEFFICIENT_BITS: u32 = 6;
/// Rounding term of the XA predictor, half of 1/64 (psx-spx: `+32`).
const XA_ROUNDING: i32 = 32;
/// Nibbles are left-aligned to bit 15 before the shift (psx-spx: "left-shifting them by 12").
const NIBBLE_ALIGN: u32 = 12;

/// The two previous output samples of a decode chain.
///
/// # Examples
///
/// A chain starts silent; each decoded sample becomes `prev1`.
///
/// ```
/// use psx_formats::adpcm::{BLOCK_BYTES, BLOCK_SAMPLES, State, decode_block};
///
/// let mut state = State::default();
/// let mut out = [0i16; BLOCK_SAMPLES];
/// decode_block(&[0u8; BLOCK_BYTES], &mut out, &mut state);
/// assert_eq!((state.prev1, state.prev2), (0, 0));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    /// Most recent sample.
    pub prev1: i16,
    /// The sample before `prev1`.
    pub prev2: i16,
}

/// How the prediction term is rounded.
///
/// # Examples
///
/// ```
/// use psx_formats::adpcm::Rounding;
///
/// let spu = Rounding::TruncateEachTerm;
/// let xa = Rounding::RoundSum;
/// assert_ne!(spu, xa);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// SPU voices: each term is shifted right on its own, no rounding.
    TruncateEachTerm,
    /// CD-ROM XA: the two terms are summed, `+32`, then shifted.
    RoundSum,
}

fn clamp16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Decode one 4-bit `nibble` with the given `shift` and `filter` (0..=4).
///
/// `shift` above 12 acts as 9. The caller guarantees `filter <= 4`.
///
/// # Examples
///
/// ```
/// use psx_formats::adpcm::{Rounding, State, step};
///
/// let mut state = State::default();
/// assert_eq!(step(7, 0, 0, &mut state, Rounding::TruncateEachTerm), 28_672);
/// ```
#[must_use]
pub fn step(nibble: u8, shift: u8, filter: usize, state: &mut State, rounding: Rounding) -> i16 {
    let shift = if shift > MAX_SHIFT { RESERVED_SHIFT } else { shift };
    let raw = (i32::from(nibble & 0x0F) << NIBBLE_ALIGN) as i16;
    let mut sample = i32::from(raw >> shift);
    let p1 = i32::from(state.prev1);
    let p2 = i32::from(state.prev2);
    match rounding {
        Rounding::TruncateEachTerm => {
            sample += (p1 * FILTER_POS[filter]) >> COEFFICIENT_BITS;
            sample += (p2 * FILTER_NEG[filter]) >> COEFFICIENT_BITS;
        }
        Rounding::RoundSum => {
            sample += (p1 * FILTER_POS[filter] + p2 * FILTER_NEG[filter] + XA_ROUNDING) >> COEFFICIENT_BITS;
        }
    }
    let out = clamp16(sample);
    state.prev2 = state.prev1;
    state.prev1 = out;
    out
}

/// Decode one 16-byte SPU block into 28 samples, advancing `state`.
///
/// Byte 0 holds shift (low nibble) and filter (high nibble; above 4 acts as 0),
/// byte 1 the flags, bytes 2..16 the nibbles, low nibble first.
///
/// # Examples
///
/// ```
/// use psx_formats::adpcm::{BLOCK_SAMPLES, State, decode_block};
///
/// let mut state = State::default();
/// let mut out = [0i16; BLOCK_SAMPLES];
/// decode_block(&[0u8; 16], &mut out, &mut state);
/// assert_eq!(out, [0i16; BLOCK_SAMPLES]);
/// ```
pub fn decode_block(block: &[u8; BLOCK_BYTES], out: &mut [i16; BLOCK_SAMPLES], state: &mut State) {
    let shift = block[0] & 0x0F;
    let filter = usize::from(block[0] >> 4);
    let filter = if filter > 4 { 0 } else { filter };
    for (i, sample) in out.iter_mut().enumerate() {
        let byte = block[2 + i / 2];
        let nibble = if i % 2 == 0 { byte & 0x0F } else { byte >> 4 };
        *sample = step(nibble, shift, filter, state, Rounding::TruncateEachTerm);
    }
}

/// Largest magnitude a nibble holds: a nibble is two's complement, -8 to 7.
const NIBBLE_LOW: i32 = -8;
const NIBBLE_HIGH: i32 = 7;

/// The predictor term of a block for `filter` at `state`, in the rounding [`step`] uses for SPU voices.
fn prediction(state: &State, filter: usize) -> i32 {
    ((i32::from(state.prev1) * FILTER_POS[filter]) >> COEFFICIENT_BITS)
        + ((i32::from(state.prev2) * FILTER_NEG[filter]) >> COEFFICIENT_BITS)
}

/// The sample that nibble `n` decodes to at `shift`, given the block's prediction. For the same nibble,
/// shift, filter and state, this is what [`step`] returns.
fn reconstruct(n: i32, shift: u8, prediction: i32) -> i16 {
    clamp16((n << (NIBBLE_ALIGN - u32::from(shift))) + prediction)
}

/// The nibble that decodes nearest `target`, with the sample it decodes to.
fn nearest(target: i16, shift: u8, prediction: i32) -> (i32, i16) {
    let scale = 1 << (NIBBLE_ALIGN - u32::from(shift));
    let below = (i32::from(target) - prediction).div_euclid(scale);
    [below, below + 1]
        .map(|n| n.clamp(NIBBLE_LOW, NIBBLE_HIGH))
        .map(|n| (n, reconstruct(n, shift, prediction)))
        .into_iter()
        .min_by_key(|&(_, sample)| (i32::from(sample) - i32::from(target)).abs())
        .expect("two candidate nibbles")
}

/// One filter and shift applied to a block: its nibbles, the samples they decode to, and the squared error.
struct Quantized {
    filter: usize,
    shift: u8,
    nibbles: [i32; BLOCK_SAMPLES],
    decoded: [i16; BLOCK_SAMPLES],
    error: i64,
}

/// Quantizes one block with one filter and shift from `state`. With `exact` set, it gives up at the first
/// sample that does not decode to its target.
fn quantize(samples: &[i16; BLOCK_SAMPLES], state: State, filter: usize, shift: u8, exact: bool) -> Option<Quantized> {
    let mut state = state;
    let mut block = Quantized { filter, shift, nibbles: [0; BLOCK_SAMPLES], decoded: [0; BLOCK_SAMPLES], error: 0 };
    for (index, &target) in samples.iter().enumerate() {
        let (n, decoded) = nearest(target, shift, prediction(&state, filter));
        if exact && decoded != target {
            return None;
        }
        let difference = i64::from(decoded) - i64::from(target);
        block.error += difference * difference;
        block.nibbles[index] = n;
        block.decoded[index] = decoded;
        state = State { prev1: decoded, prev2: state.prev1 };
    }
    Some(block)
}

/// Every filter and shift an encoder may choose, in the order the search tries them.
fn choices() -> impl Iterator<Item = (usize, u8)> {
    (0..FILTER_POS.len()).flat_map(|filter| (0..=MAX_SHIFT).map(move |shift| (filter, shift)))
}

/// Encodes 28 samples as one SPU block and advances `state` by the samples the decoder produces from it.
///
/// The block takes the first filter and shift that decode every sample exactly. A block that no filter and
/// shift reproduces takes the one with the least squared error. Byte 1, the flags, is left zero.
///
/// # Examples
///
/// ```
/// use psx_formats::adpcm::{BLOCK_SAMPLES, State, decode_block, encode_block};
///
/// let mut state = State::default();
/// let block = encode_block(&[0i16; BLOCK_SAMPLES], &mut state);
/// let mut back = [0i16; BLOCK_SAMPLES];
/// decode_block(&block, &mut back, &mut State::default());
/// assert_eq!(back, [0i16; BLOCK_SAMPLES]);
/// ```
pub fn encode_block(samples: &[i16; BLOCK_SAMPLES], state: &mut State) -> [u8; BLOCK_BYTES] {
    let block = choices()
        .find_map(|(filter, shift)| quantize(samples, *state, filter, shift, true))
        .or_else(|| {
            choices()
                .filter_map(|(filter, shift)| quantize(samples, *state, filter, shift, false))
                .min_by_key(|choice| choice.error)
        })
        .expect("a filter and shift always quantize a block");
    let mut bytes = [0u8; BLOCK_BYTES];
    bytes[0] = ((block.filter as u8) << 4) | block.shift;
    for (index, &n) in block.nibbles.iter().enumerate() {
        bytes[2 + index / 2] |= ((n & 0x0F) as u8) << (4 * (index % 2));
    }
    *state = State { prev1: block.decoded[BLOCK_SAMPLES - 1], prev2: block.decoded[BLOCK_SAMPLES - 2] };
    bytes
}

/// Encodes PCM as SPU blocks, padding the last block with zeros. The last block carries [`FLAG_END`]; loop
/// flags are not set. [`decode_stream`] reads the result back.
///
/// # Examples
///
/// ```
/// use psx_formats::adpcm::{BLOCK_BYTES, encode_stream};
///
/// let stream = encode_stream(&[0; 30]);
/// assert_eq!(stream.len(), 2 * BLOCK_BYTES);
/// assert_eq!(stream[BLOCK_BYTES + 1], psx_formats::adpcm::FLAG_END);
/// ```
pub fn encode_stream(samples: &[i16]) -> Vec<u8> {
    let mut state = State::default();
    let mut bytes = Vec::with_capacity(samples.len().div_ceil(BLOCK_SAMPLES) * BLOCK_BYTES);
    for chunk in samples.chunks(BLOCK_SAMPLES) {
        let mut block = [0i16; BLOCK_SAMPLES];
        block[..chunk.len()].copy_from_slice(chunk);
        bytes.extend_from_slice(&encode_block(&block, &mut state));
    }
    if let Some(last) = bytes.len().checked_sub(BLOCK_BYTES) {
        bytes[last + 1] = FLAG_END;
    }
    bytes
}

/// Decodes a whole SPU stream to PCM, 28 samples per block. A trailing partial block is ignored.
///
/// # Examples
///
/// ```
/// use psx_formats::adpcm::{BLOCK_BYTES, BLOCK_SAMPLES, decode_stream};
///
/// assert_eq!(decode_stream(&[0u8; 2 * BLOCK_BYTES]).len(), 2 * BLOCK_SAMPLES);
/// ```
pub fn decode_stream(bytes: &[u8]) -> Vec<i16> {
    let mut state = State::default();
    let mut pcm = Vec::with_capacity(bytes.len() / BLOCK_BYTES * BLOCK_SAMPLES);
    for block in bytes.as_chunks::<BLOCK_BYTES>().0 {
        let mut out = [0i16; BLOCK_SAMPLES];
        decode_block(block, &mut out, &mut state);
        pcm.extend_from_slice(&out);
    }
    pcm
}

#[cfg(test)]
mod encode_tests {
    use super::*;

    /// A stream whose blocks are arbitrary, so the decoded PCM is an ADPCM image.
    fn arbitrary_stream(blocks: usize) -> Vec<u8> {
        (0..blocks * BLOCK_BYTES).map(|index| (index as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3]).collect()
    }

    #[test]
    fn decoded_adpcm_encodes_back_to_the_same_pcm() {
        let stream = arbitrary_stream(200);
        let pcm = decode_stream(&stream);
        let again = encode_stream(&pcm);
        assert_eq!(decode_stream(&again), pcm);
    }

    #[test]
    fn arbitrary_pcm_takes_a_stable_second_round_trip() {
        let pcm: Vec<i16> = (0..1000).map(|index| ((index as f64 * 0.07).sin() * 20_000.0) as i16).collect();
        let first = decode_stream(&encode_stream(&pcm));
        let second = decode_stream(&encode_stream(&first));
        assert_eq!(second, first);
        assert_eq!(first.len(), pcm.len().div_ceil(BLOCK_SAMPLES) * BLOCK_SAMPLES);
    }

    #[test]
    fn the_last_block_carries_the_end_flag() {
        let bytes = encode_stream(&[0; 40]);
        assert_eq!(bytes.len(), 2 * BLOCK_BYTES);
        assert_eq!(bytes[BLOCK_BYTES + 1], FLAG_END);
        assert_eq!(bytes[1], 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden vector: filter 0 passes the shifted nibbles through.
    #[test]
    fn filter_zero_is_plain_shift() {
        let mut block = [0u8; BLOCK_BYTES];
        block[0] = 0x0C; // shift 12 (loudest), filter 0
        // Nibbles 7, 8 (-8), 1, F (-1), then zeros.
        block[2] = 0x87;
        block[3] = 0xF1;
        let mut out = [0i16; BLOCK_SAMPLES];
        decode_block(&block, &mut out, &mut State::default());
        assert_eq!(&out[..5], &[7, -8, 1, -1, 0]);
    }

    /// Golden vector for the filter path: shift 8, filter 1 (60/64 of the last sample).
    #[test]
    fn filter_one_golden_vector() {
        let mut block = [0u8; BLOCK_BYTES];
        block[0] = 0x18; // shift 8, filter 1
        block[2] = 0x77; // 7, 7
        block[3] = 0x00;
        let mut out = [0i16; BLOCK_SAMPLES];
        decode_block(&block, &mut out, &mut State::default());
        // 7 << 12 >> 8 = 112; then 112 + (112*60 >> 6) = 217; then 0 + (217*60 >> 6) = 203.
        assert_eq!(&out[..3], &[112, 217, 203]);
    }

    #[test]
    fn reserved_shift_acts_as_nine() {
        let mut a = State::default();
        let mut b = State::default();
        assert_eq!(
            step(5, 13, 0, &mut a, Rounding::TruncateEachTerm),
            step(5, 9, 0, &mut b, Rounding::TruncateEachTerm)
        );
    }

    #[test]
    fn output_saturates() {
        let mut state = State { prev1: i16::MAX, prev2: 0 };
        assert_eq!(step(7, 0, 1, &mut state, Rounding::TruncateEachTerm), i16::MAX);
    }

    /// 1013 * 60 / 64 = 949.69: each-term truncation gives 949, the XA rounded sum 950.
    #[test]
    fn rounding_modes_differ_on_the_fraction() {
        let start = State { prev1: 1013, prev2: 0 };
        assert_eq!(step(0, 0, 1, &mut { start }, Rounding::TruncateEachTerm), 949);
        assert_eq!(step(0, 0, 1, &mut { start }, Rounding::RoundSum), 950);
    }
}
