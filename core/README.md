# bof3player

The Rust library behind the player, and the `bof3player` command. It has the catalog, sources, playback and WAV, FLAC and Ogg export: `Library`, `Source`, `Player` and `Naming`.

```sh
# from the repository root
cargo test -p bof3player --all-features
```

Build the command with `cargo build --release -p bof3player --features cli --bin bof3player`. Guides: [Rust API](../docs/rust-api.md), [the command](../docs/player.md).

> A fan project, not affiliated with Capcom. *Breath of Fire* is Capcom's trademark. No game data is included; bring your own disc.
