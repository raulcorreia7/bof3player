# emi

A reader for EMI archives and their entries. `Archive::parse` and the payload reads work on bytes and need no `std`. The `std` feature adds files, extraction and entry naming.

```sh
# from the repository root
cargo test -p emi --all-features
```

Guide: [emi](../docs/crates.md#emi).

> A fan project, not affiliated with Capcom. *Breath of Fire* is Capcom's trademark. No game data is included; bring your own disc.
