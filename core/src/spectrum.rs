// SPDX-License-Identifier: MIT
//! A spectrum analyzer for the output of a [`Deck`](crate::deck::Deck): the last samples become
//! log-spaced bands in decibels. Bands rise quickly, ease down, blend with their neighbors and
//! carry peak caps; the motion follows the time between updates, so it looks the same at any
//! refresh rate. The terminal and the window draw the same bands.

use core::f32::consts::PI;
use std::time::Instant;

/// Samples analyzed per update; a power of two.
const WINDOW: usize = 2048;
/// The quietest level shown, in decibels below full scale.
const FLOOR_DB: f32 = -66.0;
/// Lowest and highest band edges in hertz.
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
/// Bands fall this much per second.
const FALL: f32 = 1.5;
/// Peak caps fall this much per second, after they leave the band.
const CAP_FALL: f32 = 0.45;
/// How fast a band closes on a louder level: the larger, the snappier, per second.
const ATTACK: f32 = 40.0;
/// The fallback time step, in seconds, for the first update.
const FIRST_STEP: f32 = 1.0 / 30.0;

/// Band levels from `0.0` to `1.0`, and the caps that trail them.
#[derive(Debug, Clone)]
pub struct Spectrum {
    /// The smoothed level of each band, low to high.
    pub levels: Vec<f32>,
    /// The recent maximum of each band.
    pub caps: Vec<f32>,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    last: Option<Instant>,
}

impl Spectrum {
    /// An analyzer with `bands` bands, all silent.
    #[must_use]
    pub fn new(bands: usize) -> Self {
        let bands = bands.max(1);
        let window = (0..WINDOW).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / WINDOW as f32).cos()).collect();
        Self {
            levels: vec![0.0; bands],
            caps: vec![0.0; bands],
            window,
            re: vec![0.0; WINDOW],
            im: vec![0.0; WINDOW],
            last: None,
        }
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
        let take = scope.len().min(WINDOW);
        let tail = &scope[scope.len() - take..];
        self.re.fill(0.0);
        self.im.fill(0.0);
        for (i, sample) in tail.iter().enumerate() {
            self.re[WINDOW - take + i] = sample * self.window[WINDOW - take + i];
        }
        fft(&mut self.re, &mut self.im);
        let bands = self.levels.len();
        let nyquist = rate as f32 / 2.0;
        let high = HIGH_HZ.min(nyquist * 0.98);
        let bin_hz = rate as f32 / WINDOW as f32;
        let mut target = vec![0.0f32; bands];
        for (band, slot) in target.iter_mut().enumerate() {
            let edge = |at: usize| LOW_HZ * (high / LOW_HZ).powf(at as f32 / bands as f32);
            let first = (edge(band) / bin_hz).floor() as usize;
            let last = ((edge(band + 1) / bin_hz).ceil() as usize).max(first + 1).min(WINDOW / 2 - 1);
            let peak = (first.min(last)..=last)
                .map(|bin| (self.re[bin] * self.re[bin] + self.im[bin] * self.im[bin]).sqrt())
                .fold(0.0f32, f32::max);
            // A full-scale sine reaches `WINDOW / 4` through a Hann window. Higher bands get a
            // gentle lift, as music falls off with frequency.
            let tilt = 1.0 + 1.5 * band as f32 / bands as f32;
            let db = 20.0 * (peak * tilt / (WINDOW as f32 / 4.0)).max(1e-9).log10();
            *slot = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        }
        let blended: Vec<f32> = (0..bands)
            .map(|band| {
                let before = target[band.saturating_sub(1)];
                let after = target[(band + 1).min(bands - 1)];
                (0.25 * before + 0.5 * target[band] + 0.25 * after).max(target[band] * 0.9)
            })
            .collect();
        let rise = 1.0 - (-ATTACK * step).exp();
        for ((level, cap), &goal) in self.levels.iter_mut().zip(&mut self.caps).zip(&blended) {
            *level = if goal > *level { *level + (goal - *level) * rise } else { (*level - FALL * step).max(goal) };
            *cap = (*level).max(*cap - CAP_FALL * step);
        }
    }

    /// Let every band fall, for when nothing plays.
    pub fn fade(&mut self) {
        let step = self.step();
        for (level, cap) in self.levels.iter_mut().zip(&mut self.caps) {
            *level = (*level - FALL * step).max(0.0);
            *cap = (*cap - CAP_FALL * step).max(*level);
        }
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

    #[test]
    fn a_sine_lights_the_band_that_holds_its_pitch() {
        let rate = 44_100;
        let scope: Vec<f32> = (0..WINDOW).map(|i| (2.0 * PI * 1000.0 * i as f32 / rate as f32).sin() * 0.8).collect();
        let mut spectrum = Spectrum::new(32);
        spectrum.advance(&scope, rate, 0.2);
        let loudest = (0..32).max_by(|&a, &b| spectrum.levels[a].total_cmp(&spectrum.levels[b])).unwrap();
        let center = LOW_HZ * (HIGH_HZ / LOW_HZ).powf((loudest as f32 + 0.5) / 32.0);
        assert!((center / 1000.0).log2().abs() < 0.3, "band centered at {center} Hz");
        assert!(spectrum.levels[loudest] > 0.7);
    }

    #[test]
    fn silence_stays_dark_and_levels_fall_slowly() {
        let mut spectrum = Spectrum::new(8);
        spectrum.update(&[0.0; 100], 44_100);
        assert!(spectrum.levels.iter().all(|&level| level == 0.0));
        spectrum.levels[3] = 1.0;
        spectrum.fade();
        assert!(spectrum.levels[3] > 0.9 && spectrum.caps[3] >= spectrum.levels[3]);
    }
}
