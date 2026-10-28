// SPDX-License-Identifier: MIT
//! The game's music and sample engines behind a `psx_audio::Player`.
//!
//! `psx-audio` owns [`Player`], [`Engine`] and [`Loop`]; the driver engines implement `Engine`
//! here. XA streams and CD-DA tracks use the `psx-audio` engines directly.

pub use psx_audio::player::{Engine, Loop, Player, XaStream};

use crate::driver::{self, MusicEngine, SampleEngine};
use crate::error::Result;
use crate::format::VAB_SAMPLE_RATE;

impl Engine for MusicEngine {
    fn render(&mut self, out: &mut [i16]) -> Result<usize> {
        MusicEngine::render(self, out)
    }
    fn seek(&mut self, seconds: f64) -> Result<()> {
        MusicEngine::seek(self, seconds)
    }
    fn rate(&self) -> u32 {
        VAB_SAMPLE_RATE
    }
    fn channels(&self) -> u32 {
        driver::CHANNELS
    }
    fn duration(&self) -> f64 {
        MusicEngine::duration(self)
    }
    fn position(&self) -> f64 {
        MusicEngine::position(self)
    }
    fn loop_window(&self) -> Option<(f64, f64)> {
        MusicEngine::loop_window(self)
    }
    fn set_loop(&mut self, mode: Loop) {
        MusicEngine::set_loop(self, mode);
    }
}

impl Engine for SampleEngine {
    fn render(&mut self, out: &mut [i16]) -> Result<usize> {
        SampleEngine::render(self, out)
    }
    fn seek(&mut self, seconds: f64) -> Result<()> {
        SampleEngine::seek(self, seconds)
    }
    fn rate(&self) -> u32 {
        VAB_SAMPLE_RATE
    }
    fn channels(&self) -> u32 {
        driver::CHANNELS
    }
    fn duration(&self) -> f64 {
        SampleEngine::duration(self)
    }
    fn position(&self) -> f64 {
        SampleEngine::position(self)
    }
}
