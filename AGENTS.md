# Agent guide

bof3player plays the music and sound of *Breath of Fire III* (PS1) from the user's own disc image.
It is a Rust workspace (`core`, `emi`, `psx-audio`, `psx-cd`, `psx-formats`) with a CLI and a
Rust library. Read [README.md](README.md) first, then [CONTRIBUTING.md](CONTRIBUTING.md)
and the page in [docs/](docs/) for the area you change.

## Build and test

Rust 1.88 or newer, edition 2024. Run from this folder:

```sh
cargo build --release -p bof3player --features cli --bin bof3player
cargo test --workspace --all-features
```

## Gate

Run before every report; CI runs the same checks:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo test --workspace --all-features
cargo check -p psx-formats -p emi -p psx-audio -p bof3player --no-default-features
cargo deny check
```

## Rules

- The repository holds no game data, extracted assets or game text. The only titles are in
  `data/bof3-names.json`, each with a source. Tests use
  constants, generated fixtures or a temporary folder (`target/tmp/<test>`).
- A task's `writes` list is the grant. Anything else is a `needs:` line in the report.
- Commit as `type(scope): subject`, imperative and lowercase. Add no AI attribution. Never amend,
  force-push or rewrite history.
- Prose is plain and current: present tense, no history.
