// SPDX-License-Identifier: MIT
//! A spectrum analyzer for the output of a [`Deck`](crate::deck::Deck): the last samples go
//! through a Hann window and an FFT and become log-spaced bands in decibels. The band count
//! follows the display (one band per terminal column, a fixed count in the window); the frequency
//! mapping and the motion are the same for every count. Bands rise fast, jump past their recent
//! average on an onset, fall at a steady rate and carry falling peak caps; the motion follows the time between updates, so it looks the
//! same at any refresh rate. A [`Tint`] follows the music too: where its energy sits, how loud
//! it is and its onsets, for the displays to color the bars with.

use core::f32::consts::PI;
use core::ops::Range;
use std::time::Instant;

use crate::theme;

/// Samples analyzed per update; a power of two. 4096 samples give bins about 11 Hz wide at
/// 44.1 kHz: finer bass, at the cost of a window about 90 ms long.
const WINDOW: usize = 4096;
/// The quietest level shown, in decibels below full scale: the foot of a bar. Lower shows
/// quieter detail but lifts the noise.
const FLOOR_DB: f32 = -70.0;
/// Lowest and highest band edges in hertz. The top edge drops to the Nyquist frequency at low
/// sample rates.
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 16_000.0;
/// Lift per octave above `LOW_HZ`, in decibels. Music has less energy high up; the lift keeps
/// the treble visible.
const TILT_DB_PER_OCTAVE: f32 = 1.0;
/// Seconds a band takes to close 63% of the way to a louder level. Smaller is snappier, larger
/// is smoother.
const ATTACK_SECONDS: f32 = 0.008;
/// Seconds a band's recent average takes to follow 63% of a change in its level. Onsets are
/// measured against it, so longer keeps an onset's lift for longer.
const AVERAGE_SECONDS: f32 = 0.25;
/// Share of a band's rise above its recent average added on top: drum hits and note attacks jump
/// out, steady tones do not. `0.0` turns the lift off.
const ONSET_BOOST: f32 = 0.5;
/// Full heights per second a band falls toward a quieter level. Larger is more responsive,
/// smaller lingers.
const FALL_PER_SECOND: f32 = 1.8;
/// Full heights per second a peak cap falls once its band drops below it. Smaller holds the
/// peaks longer.
const CAP_FALL_PER_SECOND: f32 = 0.4;
/// Seconds the tint takes to follow 63% of a change in the music. Longer is calmer.
const TINT_SECONDS: f32 = 0.4;
/// The band position, from `0.0` (lowest) to `1.0` (highest), of a balanced mix's energy: the
/// tint is neutral there, warmer below and cooler above.
const BALANCE: f32 = 0.45;
/// Warmth per unit of distance from `BALANCE`; larger swings the hue further for the same mix.
const WARMTH_GAIN: f32 = 4.0;
/// The most a bar's color moves toward `SPECTRUM_WARM` or `SPECTRUM_COOL`.
const WARMTH_MIX: f32 = 0.55;
/// The most loudness lightens the bars toward white.
const GLOW_MIX: f32 = 0.2;
/// The most an onset flash lightens the bars toward white.
const FLASH_MIX: f32 = 0.35;
/// Flash per unit of the bands' mean rise above their averages; larger flashes on softer onsets.
const FLASH_GAIN: f32 = 6.0;
/// Flash lost per second, so a flash lasts about a quarter second.
const FLASH_FALL_PER_SECOND: f32 = 4.0;
/// The fallback time step, in seconds, for the first update.
const FIRST_STEP: f32 = 1.0 / 60.0;

/// Band levels from `0.0` to `1.0`, and the caps that trail them.
#[derive(Debug, Clone)]
pub struct Spectrum {
    /// The smoothed level of each band, low to high.
    pub levels: Vec<f32>,
    /// The recent maximum of each band.
    pub caps: Vec<f32>,
    /// The colors' tint, following the music.
    pub tint: Tint,
    /// The recent average of each band's unsmoothed level.
    averages: Vec<f32>,
    /// Where each band reads the FFT, laid out for `rate`.
    bands: Vec<Band>,
    rate: u32,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    last: Option<Instant>,
}

/// Where one band reads the FFT magnitudes, and its lift in decibels.
#[derive(Debug, Clone, PartialEq)]
struct Band {
    bins: Bins,
    tilt_db: f32,
}

#[derive(Debug, Clone, PartialEq)]
enum Bins {
    /// A band narrower than one bin: the magnitude interpolated at this fractional bin, so
    /// neighboring bass bands differ instead of repeating one bin.
    At(f32),
    /// A band one bin wide or more: the loudest bin of the range.
    Peak(Range<usize>),
}

impl Band {
    /// The band's level from `0.0` (at `FLOOR_DB` or below) to `1.0` (full scale), from bin
    /// magnitudes where a full-scale sine reads `1.0`.
    fn level(&self, magnitudes: &[f32]) -> f32 {
        let magnitude = match &self.bins {
            Bins::At(at) => {
                let (bin, t) = (*at as usize, at.fract());
                magnitudes[bin] + (magnitudes[bin + 1] - magnitudes[bin]) * t
            }
            Bins::Peak(range) => magnitudes[range.clone()].iter().fold(0.0, |peak, &m| m.max(peak)),
        };
        let db = 20.0 * magnitude.max(1e-9).log10() + self.tilt_db;
        (1.0 - db / FLOOR_DB).clamp(0.0, 1.0)
    }
}

/// `count` log-spaced bands from `LOW_HZ` to `HIGH_HZ` at `rate` hertz, into `bands`.
fn layout(count: usize, rate: u32, bands: &mut Vec<Band>) {
    let bin_hz = rate as f32 / WINDOW as f32;
    let top = HIGH_HZ.min(rate as f32 / 2.0);
    let edge = |at: usize| LOW_HZ * (top / LOW_HZ).powf(at as f32 / count as f32) / bin_hz;
    bands.clear();
    bands.extend((0..count).map(|band| {
        let (low, high) = (edge(band), edge(band + 1));
        let center = (low * high).sqrt();
        let bins =
            if high - low < 1.0 { Bins::At(center) } else { Bins::Peak(low.round() as usize..high.round() as usize) };
        Band { bins, tilt_db: TILT_DB_PER_OCTAVE * (center * bin_hz / LOW_HZ).log2() }
    }));
}

impl Spectrum {
    /// An analyzer with `bands` bands, all silent.
    #[must_use]
    pub fn new(bands: usize) -> Self {
        let window = (0..WINDOW).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / WINDOW as f32).cos()).collect();
        Self {
            levels: vec![0.0; bands],
            caps: vec![0.0; bands],
            tint: Tint::default(),
            averages: vec![0.0; bands],
            bands: Vec::new(),
            rate: 0,
            window,
            re: vec![0.0; WINDOW],
            im: vec![0.0; WINDOW],
            last: None,
        }
    }

    /// Use `bands` bands from now on. The current levels, caps and averages stretch to the new count, so a
    /// resized display does not flash.
    pub fn resize(&mut self, bands: usize) {
        let old = self.levels.len();
        if bands == old {
            return;
        }
        let stretch =
            |values: &[f32]| (0..bands).map(|band| values.get(band * old / bands).copied().unwrap_or(0.0)).collect();
        self.levels = stretch(&self.levels);
        self.caps = stretch(&self.caps);
        self.averages = stretch(&self.averages);
    }

    /// Seconds since the previous update, kept between one millisecond and a tenth of a second.
    fn step(&mut self) -> f32 {
        let now = Instant::now();
        let seconds = self.last.replace(now).map_or(FIRST_STEP, |before| (now - before).as_secs_f32());
        seconds.clamp(0.001, 0.1)
    }

    /// Analyze the newest samples of `scope`, a mono mix at `rate` hertz. Fewer than a window of
    /// samples count as silence before them.
    pub fn update(&mut self, scope: &[f32], rate: u32) {
        let step = self.step();
        self.advance(scope, rate, step);
    }

    /// [`update`](Self::update) with the time step given.
    fn advance(&mut self, scope: &[f32], rate: u32, step: f32) {
        if self.rate != rate || self.bands.len() != self.levels.len() {
            layout(self.levels.len(), rate, &mut self.bands);
            self.rate = rate;
        }
        self.transform(scope);
        let rise = 1.0 - (-step / ATTACK_SECONDS).exp();
        let follow = 1.0 - (-step / AVERAGE_SECONDS).exp();
        let (mut loudness, mut weight, mut position, mut onset) = (0.0, 0.0, 0.0, 0.0);
        let bands = self.levels.iter_mut().zip(&mut self.caps).zip(&mut self.averages);
        for (at, (band, ((level, cap), average))) in self.bands.iter().zip(bands).enumerate() {
            let goal = band.level(&self.re);
            *average += (goal - *average) * follow;
            let jump = (goal - *average).max(0.0);
            loudness += goal;
            weight += goal * goal;
            position += goal * goal * at as f32;
            onset += jump;
            let goal = (goal + ONSET_BOOST * jump).min(1.0);
            *level = if goal > *level {
                *level + (goal - *level) * rise
            } else {
                (*level - FALL_PER_SECOND * step).max(goal)
            };
            *cap = level.max(*cap - CAP_FALL_PER_SECOND * step);
        }
        let count = self.levels.len().max(1) as f32;
        let centroid = if weight > 0.0 { position / weight / (count - 1.0).max(1.0) } else { BALANCE };
        let goal = Tint {
            warmth: ((BALANCE - centroid) * WARMTH_GAIN).clamp(-1.0, 1.0),
            glow: loudness / count,
            flash: (onset / count * FLASH_GAIN).min(1.0),
        };
        self.tint.follow(goal, step);
    }

    /// Window the newest `WINDOW` samples of `scope` and leave the magnitude of each bin up to
    /// the Nyquist bin in `re`, scaled so a full-scale sine reads `1.0`.
    fn transform(&mut self, scope: &[f32]) {
        let take = scope.len().min(WINDOW);
        let start = WINDOW - take;
        self.re[..start].fill(0.0);
        for ((re, &sample), &weight) in
            self.re[start..].iter_mut().zip(&scope[scope.len() - take..]).zip(&self.window[start..])
        {
            *re = sample * weight;
        }
        self.im.fill(0.0);
        fft(&mut self.re, &mut self.im);
        // A full-scale sine peaks at `WINDOW / 4` through a Hann window.
        let scale = 4.0 / WINDOW as f32;
        for (re, &im) in self.re[..=WINDOW / 2].iter_mut().zip(&self.im) {
            *re = (*re * *re + im * im).sqrt() * scale;
        }
    }

    /// Let every band fall, for when nothing plays.
    pub fn fade(&mut self) {
        let step = self.step();
        let bands = self.levels.iter_mut().zip(&mut self.caps).zip(&mut self.averages);
        for ((level, cap), average) in bands {
            *level = (*level - FALL_PER_SECOND * step).max(0.0);
            *cap = (*cap - CAP_FALL_PER_SECOND * step).max(*level);
            *average = average.min(*level);
        }
        self.tint.follow(Tint::default(), step);
    }
}

/// How the music colors the spectrum: three values, smoothed over time so the colors never
/// flicker. The default tint leaves the theme's colors as they are; a paused deck settles back to
/// it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tint {
    /// From `-1.0` (the energy sits in the treble: cooler) to `1.0` (in the bass: warmer).
    pub warmth: f32,
    /// Loudness from `0.0` to `1.0`: louder music glows brighter.
    pub glow: f32,
    /// A short flash after an onset, from `0.0` to `1.0`.
    pub flash: f32,
}

impl Tint {
    /// `rgb`, one of the spectrum's theme colors, tinted: moved toward the warm or the cool tint,
    /// then lightened by the glow and the flash.
    #[must_use]
    pub fn apply(self, rgb: u32) -> u32 {
        let toward = if self.warmth > 0.0 { theme::SPECTRUM_WARM } else { theme::SPECTRUM_COOL };
        let tinted = theme::mix(rgb, toward, self.warmth.abs() * WARMTH_MIX);
        theme::mix(tinted, 0xffffff, self.glow * GLOW_MIX + self.flash * FLASH_MIX)
    }

    /// Ease toward `goal` over `TINT_SECONDS`. The flash rises at once and falls at a steady rate.
    fn follow(&mut self, goal: Tint, step: f32) {
        let t = 1.0 - (-step / TINT_SECONDS).exp();
        self.warmth += (goal.warmth - self.warmth) * t;
        self.glow += (goal.glow - self.glow) * t;
        self.flash = goal.flash.max(self.flash - FLASH_FALL_PER_SECOND * step);
    }
}

/// An in-place radix-2 FFT of `re` and `im`, whose length is a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f32;
        let (step_re, step_im) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut w_re, mut w_im) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let t_re = re[b] * w_re - im[b] * w_im;
                let t_im = re[b] * w_im + im[b] * w_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let next = w_re * step_re - w_im * step_im;
                w_im = w_re * step_im + w_im * step_re;
                w_re = next;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    fn sine(hz: f32, amplitude: f32) -> Vec<f32> {
        (0..WINDOW).map(|i| (2.0 * PI * hz * i as f32 / RATE as f32).sin() * amplitude).collect()
    }

    /// The center frequency, in hertz, of the loudest of `count` bands after a long look at `scope`.
    fn loudest_center(scope: &[f32], count: usize) -> f32 {
        let mut spectrum = Spectrum::new(count);
        spectrum.advance(scope, RATE, 10.0);
        let loudest = (0..count).max_by(|&a, &b| spectrum.levels[a].total_cmp(&spectrum.levels[b])).unwrap();
        assert!(spectrum.levels[loudest] > 0.6, "{:?}", spectrum.levels);
        LOW_HZ * (HIGH_HZ / LOW_HZ).powf((loudest as f32 + 0.5) / count as f32)
    }

    #[test]
    fn bands_are_log_spaced_and_each_reads_its_own_bins() {
        let mut bands = Vec::new();
        for count in [16, 64, 120, 256] {
            layout(count, RATE, &mut bands);
            assert_eq!(bands.len(), count);
            let mut previous: Option<&Bins> = None;
            for band in &bands {
                match (&band.bins, previous) {
                    (Bins::At(at), Some(Bins::At(before))) => assert!(at > before),
                    (Bins::Peak(range), Some(Bins::Peak(before))) => assert_eq!(range.start, before.end),
                    (Bins::Peak(range), _) => assert!(!range.is_empty()),
                    (Bins::At(_), Some(Bins::Peak(_))) => panic!("a narrow band above a wide one"),
                    (Bins::At(at), None) => assert!(*at >= LOW_HZ * WINDOW as f32 / RATE as f32),
                }
                previous = Some(&band.bins);
            }
            assert!(matches!(&bands[count - 1].bins, Bins::Peak(range) if range.end <= WINDOW / 2));
        }
    }

    #[test]
    fn the_top_band_stays_below_nyquist_at_low_rates() {
        let mut bands = Vec::new();
        layout(64, 22_050, &mut bands);
        assert!(matches!(&bands[63].bins, Bins::Peak(range) if range.end <= WINDOW / 2));
    }

    #[test]
    fn a_sine_lights_the_band_that_holds_its_pitch_at_any_band_count() {
        for hz in [60.0, 250.0, 1000.0, 8000.0] {
            for count in [32, 100, 256] {
                let center = loudest_center(&sine(hz, 0.1), count);
                assert!((center / hz).log2().abs() < 0.3, "{hz} Hz lit the band centered at {center} Hz of {count}");
            }
        }
    }

    #[test]
    fn narrow_bass_bands_differ_instead_of_repeating_one_bin() {
        let mut spectrum = Spectrum::new(200);
        spectrum.advance(&sine(80.0, 0.1), RATE, 10.0);
        let narrow = spectrum.bands.iter().take_while(|band| matches!(band.bins, Bins::At(_))).count();
        assert!(narrow > 10);
        let lit = spectrum.levels[..narrow].iter().take_while(|&&level| level > 0.0).count();
        assert!(lit > 10);
        assert!(spectrum.levels[..lit].windows(2).all(|pair| pair[0] != pair[1]), "{:?}", &spectrum.levels[..lit]);
    }

    #[test]
    fn silence_stays_dark_and_levels_fall_at_a_steady_rate() {
        let mut spectrum = Spectrum::new(8);
        spectrum.update(&[0.0; 100], RATE);
        assert!(spectrum.levels.iter().all(|&level| level == 0.0));
        spectrum.levels[3] = 1.0;
        spectrum.caps[3] = 1.0;
        spectrum.advance(&[0.0; 100], RATE, 0.1);
        assert!((spectrum.levels[3] - (1.0 - FALL_PER_SECOND * 0.1)).abs() < 1e-6);
        assert!((spectrum.caps[3] - (1.0 - CAP_FALL_PER_SECOND * 0.1)).abs() < 1e-6);
    }

    #[test]
    fn an_onset_jumps_past_the_settled_level_then_eases_back() {
        let tone = sine(1000.0, 0.05);
        let mut settled = Spectrum::new(32);
        settled.advance(&tone, RATE, 10.0);
        let loudest = (0..32).max_by(|&a, &b| settled.levels[a].total_cmp(&settled.levels[b])).unwrap();
        let mut spectrum = Spectrum::new(32);
        spectrum.advance(&tone, RATE, 5.0 * ATTACK_SECONDS);
        assert!(
            spectrum.levels[loudest] > settled.levels[loudest] + 0.1,
            "{} against {}",
            spectrum.levels[loudest],
            settled.levels[loudest]
        );
        for _ in 0..200 {
            spectrum.advance(&tone, RATE, 0.01);
        }
        assert!((spectrum.levels[loudest] - settled.levels[loudest]).abs() < 0.01);
    }

    #[test]
    fn motion_is_the_same_at_any_frame_rate() {
        for scope in [sine(1000.0, 0.8), vec![0.0; WINDOW]] {
            let mut once = Spectrum::new(32);
            once.levels.fill(0.5);
            once.caps.fill(0.9);
            let mut twice = once.clone();
            for _ in 0..10 {
                once.advance(&scope, RATE, 0.02);
                twice.advance(&scope, RATE, 0.01);
                twice.advance(&scope, RATE, 0.01);
            }
            for (a, b) in once.levels.iter().chain(&once.caps).zip(twice.levels.iter().chain(&twice.caps)) {
                assert!((a - b).abs() < 0.02, "{a} against {b}");
            }
        }
    }

    #[test]
    fn the_tint_warms_with_bass_cools_with_treble_and_settles_when_paused() {
        let mut bass = Spectrum::new(64);
        bass.advance(&sine(80.0, 0.3), RATE, 10.0);
        let mut treble = Spectrum::new(64);
        treble.advance(&sine(6000.0, 0.3), RATE, 10.0);
        assert!(bass.tint.warmth > 0.5 && treble.tint.warmth < -0.5, "{:?} {:?}", bass.tint, treble.tint);
        assert!(bass.tint.glow > 0.0);
        let red = |rgb: u32| (rgb >> 16) & 0xff;
        let base = theme::SPECTRUM[1];
        assert!(red(bass.tint.apply(base)) > red(base) && red(treble.tint.apply(base)) < red(base));
        assert_eq!(Tint::default().apply(base), base);
        // Two seconds of first-update steps.
        for _ in 0..120 {
            bass.last = None;
            bass.fade();
        }
        assert!(bass.tint.warmth.abs() < 0.01 && bass.tint.glow < 0.01 && bass.tint.flash == 0.0);
    }

    #[test]
    fn an_onset_flashes_then_the_flash_fades() {
        let mut spectrum = Spectrum::new(32);
        spectrum.advance(&sine(1000.0, 0.3), RATE, ATTACK_SECONDS);
        assert!(spectrum.tint.flash > 0.0);
        let lit = Tint { flash: 1.0, ..Tint::default() };
        assert!(lit.apply(theme::SPECTRUM[0]) != theme::SPECTRUM[0]);
        for _ in 0..50 {
            spectrum.advance(&sine(1000.0, 0.3), RATE, 0.02);
        }
        assert!(spectrum.tint.flash < 0.05);
    }

    #[test]
    fn resize_stretches_the_current_levels() {
        let mut spectrum = Spectrum::new(2);
        spectrum.levels = vec![0.25, 0.75];
        spectrum.resize(4);
        assert_eq!(spectrum.levels, [0.25, 0.25, 0.75, 0.75]);
        assert_eq!(spectrum.caps.len(), 4);
        spectrum.advance(&sine(1000.0, 0.8), RATE, 0.1);
        assert_eq!(spectrum.bands.len(), 4);
    }
}
