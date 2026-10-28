# psx-formats

Decoders, encoders and validators for PlayStation file formats: SPU and XA ADPCM, VAB sound banks, SEP sequence packages, Mode 2 Form 1 sectors, RIFF/WAVE, and the GPU's VRAM uploads, CLUTs, TIM images and PNG views. Without `std` the crate is `no_std` plus `alloc`. It knows no game or container.

```sh
# from the repository root
cargo test -p psx-formats --all-features
```

Guide: [psx-formats](../docs/crates.md#psx-formats).

> A fan project, not affiliated with Capcom. *Breath of Fire* is Capcom's trademark. No game data is included; bring your own disc.
