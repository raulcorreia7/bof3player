# Changelog

All notable changes to `psx-formats` are recorded here. The crate follows
[Semantic Versioning](https://semver.org/).

## 0.1.2

- Maintenance release.

## 0.1.0

First release, as its own crate.

- ADPCM: the 16-byte SPU block and the CD-ROM XA predictor, block and stream
  decoders, and an encoder.
- XA: stored-sector scan, channel spans, and a channel decoder.
- Sectors: Mode 2 Form 1 EDC and parity, sealing and checking.
- VAB: header tables, validator, encoder, and sample decoding.
- SEP: package parser and encoder, validator, and Standard MIDI File export and
  import.
- Graphics: type-3 VRAM uploads, CLUT palettes, texture-page and CLUT words,
  TIM images, and PNG input and output.
- WAVE: the canonical 16-bit PCM writer and a shape reader.
- SoundFont 2: a writer and reader for VAB banks.
- `fixtures`: generated test banks, sequences and sectors behind a feature.
