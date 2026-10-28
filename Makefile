# Build, test and check the bof3player workspace. Needs cargo; GNU make.
# Cargo's target dir is target/ here; set CARGO_TARGET_DIR to move it.

.DEFAULT_GOAL := help

# Passed to `run-tui` and `run-gui`, e.g. make run-tui ARGS='-i "Breath of Fire III.cue"'.
ARGS ?=

.PHONY: help build test format lint check run-tui run-gui clean

help:
	@echo "bof3player: the player, its library and the PlayStation crates (cargo)"
	@echo ""
	@echo "Targets:"
	@echo "  help     List these targets (default)."
	@echo "  build    Release build of bof3player, bof3player-tui and bof3player-gui."
	@echo "  test     cargo test --workspace --all-features --locked."
	@echo "  format   cargo fmt; CHECK=1 only checks."
	@echo "  lint     cargo clippy -D warnings, then cargo deny if installed."
	@echo "  check    format, lint, test and no-default-features check."
	@echo "  run-tui  Run the terminal browser; pass ARGS='...'."
	@echo "  run-gui  Run the window; pass ARGS='...'."
	@echo "  clean    cargo clean."
	@echo ""
	@echo "Without make, run these cargo lines:"
	@echo "  cargo build --release --locked -p bof3player --features cli --bin bof3player --bin bof3player-tui"
	@echo "  cargo build --release --locked -p bof3player --features gui --bin bof3player-gui"
	@echo "  cargo test --workspace --all-features --locked"

build:
	cargo build --release --locked -p bof3player --features cli --bin bof3player --bin bof3player-tui
	cargo build --release --locked -p bof3player --features gui --bin bof3player-gui

test:
	cargo test --workspace --all-features --locked

# Format all crates; CHECK=1 verifies without writing.
format:
	cargo fmt --all $(if $(CHECK),--check)

lint:
	cargo clippy --workspace --all-features --all-targets --locked -- -D warnings
	@if command -v cargo-deny >/dev/null 2>&1; then cargo deny check; \
	else echo "cargo-deny not installed: skipped"; fi

check:
	$(MAKE) format CHECK=1
	$(MAKE) lint
	$(MAKE) test
	cargo check -p psx-formats -p emi -p psx-audio -p bof3player --no-default-features --locked

run-tui:
	cargo run --release --locked -p bof3player --features cli --bin bof3player-tui -- $(ARGS)

run-gui:
	cargo run --release --locked -p bof3player --features gui --bin bof3player-gui -- $(ARGS)

clean:
	cargo clean
