# Changelog

All notable changes to `psx-audio` are recorded here. The crate follows
[Semantic Versioning](https://semver.org/).

## 0.1.0

First release, as its own crate.

- `spu`: the SPU voice engine: 24 ADPCM voices, ADSR, 4-point Gaussian
  interpolation, reverb, main volume and the psx-spx presets.
- `player`: `Player` over any `Engine`, with the XA-stream and CD-DA engines.
- `source`: `Blob` and `Source` over memory, callbacks, folders, disc images and
  any `Read + Seek`; `MemorySource`, `CallbackSource`, `FolderSource`,
  `DiscSource` and `ReaderBlob`.
- `export`: WAVE, raw little-endian samples, FLAC and Ogg Vorbis, with Vorbis
  comment tags for titles and loops.
- `error`: one `Error` type and `Result`.
