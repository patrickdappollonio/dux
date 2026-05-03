.PHONY: run fmt fmt-check lint lint-fix profiling overlay-shellcheck overlay-bats overlay-test

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
	  dux-amq/scripts/dux-amq-doctor \
	  dux-amq/config/bashrc-additions.sh

overlay-bats:
	@command -v bats >/dev/null 2>&1 || { echo "Error: 'bats' not found (apt-get install bats)"; exit 1; }
	bats dux-amq/tests

overlay-test: overlay-shellcheck overlay-bats
