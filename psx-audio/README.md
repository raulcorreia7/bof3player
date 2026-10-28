# psx-audio

PlayStation audio in Rust: an SPU voice engine, XA and CD-DA playback, file sources, and WAV, FLAC and Ogg Vorbis export. The hardware model follows psx-spx. The crate knows no game, archive container or catalog.

```sh
# from the repository root
cargo test -p psx-audio --all-features
```

Guide: [psx-audio](../docs/crates.md#psx-audio). The hardware sources are in [spu-model.md](../docs/spu-model.md).

> A fan project, not affiliated with Capcom. *Breath of Fire* is Capcom's trademark. No game data is included; bring your own disc.
