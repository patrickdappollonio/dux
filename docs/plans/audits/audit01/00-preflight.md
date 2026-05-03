# Phase 00: Preflight — verify assumptions, baseline, scaffolding

> Maps to audit findings: — (foundation for all other phases)

## Goal
Establish a known-good baseline before any production-readiness change lands.
Re-verify the audit's spot-checked facts, snapshot CI green-ness, and create
the `dux-amq/tests/` bats harness later phases depend on.

## Pre-conditions
- Clean working tree on `dux-amq-setup`.
- `cargo`, `bash`, `shellcheck`, `bats-core`, `jq`, `git` on PATH.
- `upstream` remote configured to `patrickdappollonio/dux`.

## Files to touch
- `dux-amq/tests/` — create.
- `dux-amq/tests/lib/setup.bash` — common bats helpers (tmp `$HOME`, fakes).
- `.github/workflows/overlay-ci.yml` — runs shellcheck + bats on PR.
- `tests/scrollbar_render.rs` — empty placeholder (filled in Phase 10).

## Steps
1. Re-confirm spot-checked facts in code: `codex-amq:27` unconditional
   YOLO; `claude-amq:11/26-27` doc/code mismatch; `finalize:25` `--delete`,
   `:27,:29` non-atomic; `install.sh:23` `grep -oP`.
2. Re-measure upstream drift:
   ```bash
   git fetch upstream
   git log HEAD..upstream/main --oneline | tee /tmp/upstream-drift.txt
   ```
   Audit said 1 commit; at plan authoring it is **7**. Save the file as
   the artifact Phase 06 references.
3. Snapshot baseline tests green:
   ```bash
   cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
   shellcheck dux-amq/install.sh dux-amq/wrappers/* dux-amq/scripts/*.sh
   ```
   Resolve any failure before proceeding.
4. Add `dux-amq/tests/lib/setup.bash` with the helpers Phases 02/03/04/12
   will use:
   ```bash
   setup_isolated_home() {
     export TEST_HOME=$(mktemp -d); export HOME="$TEST_HOME"
     export PATH="$BATS_TEST_DIRNAME/fakes:$PATH"
   }
   teardown_isolated_home() { rm -rf "$TEST_HOME"; }
   ```
5. Add minimal CI:
   ```yaml
   # .github/workflows/overlay-ci.yml
   name: overlay-ci
   on: [pull_request, push]
   jobs:
     shell:
       runs-on: ubuntu-24.04
       steps:
         - uses: actions/checkout@v4
         - run: sudo apt-get update && sudo apt-get install -y shellcheck bats jq
         - run: shellcheck dux-amq/install.sh dux-amq/wrappers/* dux-amq/scripts/*.sh
         - run: bats dux-amq/tests
   ```
6. Land as one PR `chore(audit01): preflight scaffolding`.

## Validation
- `gh pr checks` green on the preflight PR.
- `bats dux-amq/tests` exits 0.
- `cargo test` passes locally and in CI.

## Acceptance criteria
- [x] Four spot-checked facts re-confirmed at HEAD.
      - `dux-amq/wrappers/codex-amq:27` — unconditional
        `--dangerously-bypass-approvals-and-sandbox` confirmed.
      - `dux-amq/wrappers/claude-amq:11` doc says seeding is "OFF by
        default", but `:26-27` (`CLAUDE_AMQ_NO_SEED` opt-out) implements
        it as ON by default. Mismatch confirmed.
      - `dux-amq/scripts/finalize-claude-migration.sh:25` `rsync --delete`
        confirmed; `:27,:29` non-atomic `mv` then `ln -s` swap confirmed.
      - `dux-amq/install.sh:23` `grep -oP` confirmed.
- [x] Drift count + short hashes recorded
      (`docs/plans/audits/audit01/artifacts/00-preflight-upstream-drift.txt`,
      7 commits as expected).
- [x] `cargo fmt`, `clippy -D warnings`, `cargo test`, `shellcheck` green.
      Two pre-existing failures fixed in commit "chore(audit01): fix
      pre-existing baseline lint failures" (clippy `unnecessary_cast`
      ×2 in `src/app/render.rs`; shellcheck SC2015 in `install.sh` and
      SC2155 in `finalize-claude-migration.sh`).
- [x] `dux-amq/tests/lib/setup.bash` sources cleanly (verified by
      `bats dux-amq/tests` — 3/3 passing in `dux-amq/tests/smoke.bats`).
- [ ] `overlay-ci.yml` passes on PR. <!-- gap: cannot verify without
      pushing + opening a PR; CI run will be observed once pushed. Local
      mirror via `make overlay-test` is green. -->

## References
- `dux-amq-audit.md` lines 5–11 (drift was 1 at audit; verified 7 at plan).
- bats-core: https://bats-core.readthedocs.io/
