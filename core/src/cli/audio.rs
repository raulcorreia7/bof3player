// SPDX-License-Identifier: MIT
//! Audio files for the browsers: a decoded WAV, FLAC or Ogg file as a deck [`Source`].
//! The deck itself lives in [`crate::deck`].

use crate::player::Loop;

pub use crate::deck::{Deck, Source};

/// A WAV, FLAC or Ogg Vorbis file held in memory as interleaved f32 samples.
pub struct FileSource {
    samples: Vec<f32>,
    rate: u32,
    channels: usize,
    at: usize,
}

impl FileSource {
    /// Decode a `.wav`, `.flac` or `.ogg` file.
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let extension = path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("wav") => Self::wav(path),
            Some("flac") => Self::flac(path),
            Some("ogg") => Self::ogg(path),
            _ => Err(format!("{}: use a .wav, .flac or .ogg file", path.display())),
        }
    }

    /// Decode 16-bit or 32-bit float PCM; other WAV layouts are an error.
    fn wav(path: &std::path::Path) -> Result<Self, String> {
        let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        let spec = reader.spec();
        let samples = match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Int, 16) => {
                reader.samples::<i16>().map(|s| s.map(|v| f32::from(v) / 32768.0)).collect::<Result<Vec<_>, _>>()
            }
            (hound::SampleFormat::Float, 32) => reader.samples::<f32>().collect::<Result<Vec<_>, _>>(),
            (format, bits) => return Err(format!("unsupported WAV layout: {format:?} {bits}-bit")),
        }
        .map_err(|e| e.to_string())?;
        let channels = usize::from(spec.channels).clamp(1, 2);
        Ok(Self { samples, rate: spec.sample_rate, channels, at: 0 })
    }

    fn flac(path: &std::path::Path) -> Result<Self, String> {
        let mut reader = claxon::FlacReader::open(path).map_err(|e| e.to_string())?;
        let info = reader.streaminfo();
        if info.bits_per_sample == 0 || info.bits_per_sample > 31 {
            return Err(format!("unsupported FLAC depth: {}-bit", info.bits_per_sample));
        }
        let scale = (1u32 << (info.bits_per_sample - 1)) as f32;
        let samples = reader
            .samples()
            .map(|sample| sample.map(|value| value as f32 / scale))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(Self { samples, rate: info.sample_rate, channels: info.channels as usize, at: 0 })
    }

    fn ogg(path: &std::path::Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut reader = lewton::inside_ogg::OggStreamReader::new(file).map_err(|e| e.to_string())?;
        let channels = usize::from(reader.ident_hdr.audio_channels).clamp(1, 2);
        let rate = reader.ident_hdr.audio_sample_rate;
        let mut samples = Vec::new();
        while let Some(packet) = reader.read_dec_packet_itl().map_err(|e| e.to_string())? {
            samples.extend(packet.into_iter().map(|sample| f32::from(sample) / 32768.0));
        }
        // The granule position is the exact length; decoders may pad the last packet.
        if let Some(frames) = reader.get_last_absgp() {
            samples.truncate(frames as usize * channels);
        }
        Ok(Self { samples, rate, channels, at: 0 })
    }
}

impl Source for FileSource {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn position(&self) -> f64 {
        (self.at / self.channels) as f64 / f64::from(self.rate)
    }
    fn duration(&self) -> f64 {
        (self.samples.len() / self.channels) as f64 / f64::from(self.rate)
    }
    fn loop_window(&self) -> Option<(f64, f64)> {
        None
    }
    fn set_loop(&mut self, _mode: Loop) {}
    fn seek(&mut self, seconds: f64) {
        let frame = (seconds.max(0.0) * f64::from(self.rate)) as usize;
        self.at = (frame * self.channels).min(self.samples.len());
    }
    fn render_f32(&mut self, out: &mut [f32]) -> usize {
        let count = out.len().min(self.samples.len() - self.at);
        out[..count].copy_from_slice(&self.samples[self.at..self.at + count]);
        self.at += count;
        count / self.channels
    }
}
