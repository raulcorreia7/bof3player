# SPU model

PS1 SPU behavior implemented by the offline renderer, with evidence from public
hardware documentation. For crate boundaries and supported playback paths, see
the [bof3player README](../README.md).

## Evidence

| Evidence                                                                            | Establishes                                                                                           |
| ----------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| [psx-spx SPU specification](https://psx-spx.consoledev.net/soundprocessingunitspu/) | Hardware registers, ADPCM, pitch, interpolation, ADSR, volume, transfer, noise, modulation and reverb |

Numeric tables match their cited sources:

| Table                             | Source                                    | Check                |
| --------------------------------- | ----------------------------------------- | -------------------- |
| SPU Gaussian interpolation (512)  | psx-spx, "4-Point Gaussian Interpolation" | 512/512 values match |
| SPU reverb presets (10 × 32)      | psx-spx, "SPU Reverb Examples"            | 10/10 presets match  |
| ADPCM filter coefficients (2 × 5) | psx-spx, "Pos/neg Tables"                 | Values match         |
| Pitch table (193 values)          | `floor(0x1000 * 2^(i/192))`               | 193/193 values match |

Model constants and register behavior follow psx-spx; this crate does not copy source code from
another emulator.

## Hardware scope

The SPU advances at 44.1 kHz, once per `0x300` CPU clocks; hardware clocks voice
and main register changes per sample, while offline direct rendering schedules
events on output frames. The hardware contract covers 24 voices over 512 KiB of
SPU RAM in 8-byte address units, 16-byte ADPCM blocks of 28 samples, `VxPitch`
(`0x1000` = 44.1 kHz, clamped above `0x4000`), fixed and swept volumes,
KON/KOFF/ENDX, PMON, NON, and preset-based reverb; the decode, interpolation,
envelope, and transfer contracts follow below.

## ADPCM decode

psx-spx formula (`SPU ADPCM Samples`):

```c
sample = (int16_t)(nibble << 12) >> shift;   // sign-extend + shift
sample += (prev1 * filter_pos) >> 6;
sample += (prev2 * filter_neg) >> 6;
clamp(sample, -32768, 32767);
```

Filter coefficients:

| Filter | pos | neg |
| -----: | --: | --: |
|      0 |   0 |   0 |
|      1 |  60 |   0 |
|      2 | 115 | −52 |
|      3 |  98 | −55 |
|      4 | 122 | −60 |

Reserved shifts 13–15 decode as shift 9; reserved filters 5–15 use zero coefficients. On hardware, ADPCM predictor and Gaussian sample history continue across loop jumps instead of resetting at the loop-start block.

## Gaussian interpolation

4-point interpolation using the hardware's 512-entry Gaussian table (psx-spx `4-Point Gaussian Interpolation`):

```c
out  = (gauss[0x0FF - i] * oldest) >> 15;
out += (gauss[0x1FF - i] * older)  >> 15;
out += (gauss[0x100 + i] * old)    >> 15;
out += (gauss[0x000 + i] * new)    >> 15;
```

The interpolation index is pitch-counter bits 4–11; bits 12 and above select the decoded source sample. Steps above `0x4000` clamp to `0x4000` after optional pitch modulation, so high pitches may skip decoded samples while using the same four-point window.

## ADSR envelope

From psx-spx (`SPU Volume and ADSR Generator`):

```text
AdsrCycles = 1 << max(0, shift - 11)
AdsrStep   = step_value << max(0, 11 - shift)

if exponential AND increasing AND level > 0x6000:
    AdsrCycles *= 4    (or step /= 4 for rate < 40)

if exponential AND decreasing:
    AdsrStep = AdsrStep * level / 0x8000
```

| Phase   | Mode        | Direction | Step            |
| ------- | ----------- | --------- | --------------- |
| Attack  | Linear/Exp  | Increase  | +7,+6,+5,+4     |
| Decay   | Exp (fixed) | Decrease  | −8              |
| Sustain | Linear/Exp  | Prog      | +7..+4 / −8..−5 |
| Release | Linear/Exp  | Decrease  | −8              |

ADSR1: bit15=attack_mode, bits14-10=attack_shift, bits9-8=attack_step, bits7-4=decay_shift, bits3-0=sustain_level (`(N+1)*0x800`).

ADSR2: bit15=sustain_mode, bit14=sustain_dir, bits12-8=sustain_shift, bits7-6=sustain_step, bit5=release_mode, bits4-0=release_shift.

## Voice and transfer invariants

- KON clears the matching ENDX bit, resets ADSR level and interpolation/predictor history, and starts attack from the configured start address; writing a start address does not redirect a playing voice.
- KOFF enters release from any ADSR phase; it does not immediately silence the voice.
- ADPCM flag bit 0 sets ENDX after the current block; with repeat bit 1 set it jumps to the repeat address, otherwise playback ends and release begins. Flag bit 2 copies the current block address into the repeat address.
- Fixed volume is signed; negative values invert phase. Bit 15 selects a separate sweep envelope, not a fixed magnitude.
- SPU RAM is not CPU-mapped. Transfer and voice addresses use 8-byte units; the transfer FIFO port moves 16-bit values and the hardware FIFO holds 32 halfwords. DMA uses channel 4.
- SPUCNT controls enable/mute, transfer mode, reverb, noise clock, and CD or external input. SPUSTAT reflects several changes after hardware delay, not immediately.

PMON modulates voice `n` from voice `n-1`; voice 0 cannot be modulated. NON
replaces ADPCM with a shared hardware noise source, so `VxPitch` does not set
noise frequency. Reverb is a half-rate SPU-RAM feedback pipeline with per-voice
send bits and master enable/output controls.

## Compatibility behavior

- The SPU voice decoder truncates each predictor term; the XA decoder rounds
  the sum. `psx_formats::adpcm::Rounding` names these modes.
- The default reverb timing follows psx-spx. `ReverbModel::Legacy` preserves
  the earlier timing.

## Boundaries

- This model covers SPU voices and mixing; XA and CD-DA use separate players.
- Export and codec behavior are outside the hardware model.
