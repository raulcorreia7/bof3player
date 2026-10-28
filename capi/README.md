# bof3player-capi

The C ABI of `bof3player`: opaque library and player handles, status codes, a per-thread last-error message and a generated header, `include/bof3player.h`.

```sh
# from the repository root
cargo test -p bof3player-capi
```

Build the shared and static libraries with `cargo build --release -p bof3player-capi`. Guide: [C API](../docs/c-api.md).

> A fan project, not affiliated with Capcom. *Breath of Fire* is Capcom's trademark. No game data is included; bring your own disc.
