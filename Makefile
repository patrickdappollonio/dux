.PHONY: run fmt fmt-check lint lint-fix profiling overlay-shellcheck overlay-bats overlay-test test-all-platforms

run:
	cargo run

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint:
	cargo clippy --all-targets --all-features

lint-fix:
	cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged

profiling:
	@command -v flamegraph >/dev/null 2>&1 || { echo "Error: 'flamegraph' not found. Install it with: cargo install flamegraph"; exit 1; }
	cargo flamegraph --profile profiling --bin dux -o flamegraph.svg

# audit01 Phase 00: local mirror of `.github/workflows/overlay-ci.yml`.
# Run before pushing changes that touch dux-amq/.
overlay-shellcheck:
	@command -v shellcheck >/dev/null 2>&1 || { echo "Error: 'shellcheck' not found (apt-get install shellcheck)"; exit 1; }
	shellcheck \
	  install.sh \
	  dux-amq/install.sh \
	  dux-amq/wrappers/* \
	  dux-amq/scripts/*.sh \
	  dux-amq/config/bashrc-additions.sh

overlay-bats:
	@command -v bats >/dev/null 2>&1 || { echo "Error: 'bats' not found (apt-get install bats)"; exit 1; }
	bats dux-amq/tests

overlay-test: overlay-shellcheck overlay-bats

# audit02 Phase 21 (P1-S): cross-platform test entrypoint stub.
#
# A single host can only run native tests for the OS it's booted into,
# so this target documents that the canonical multi-platform run lives
# in CI (`pr.yml` / `test.yml` matrix on `ubuntu-24.04` + `macos-14`).
# Locally, contributors should run `cargo test --all-features` on every
# host they have access to (typically Linux + macOS).
test-all-platforms:
	@echo "Cross-platform tests run via CI matrix (ubuntu-24.04 + macos-14)."
	@echo "Locally, run 'cargo test --all-features' on each host (Linux + macOS)."
	@echo "Non-UTF-8 portability tests live in tests/git_portability.rs and"
	@echo "are gated by #[cfg(unix)] so they cover both kernels."
