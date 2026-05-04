# Phase 00: Preflight — re-verify, baseline, scaffolding

> Maps to: foundation for all phases. No audit IDs.

## Goal
Establish a known-good baseline before any audit02 change lands. Re-verify
audit02's spot-checked findings against current HEAD (audit was captured at
`554255d`; line numbers may have shifted), capture artifacts the later
phases reference, and snapshot CI green-ness.

## Pre-conditions
- Clean working tree.
- `cargo`, `bash`, `shellcheck`, `bats`, `jq`, `git`, `sha256sum` on PATH.
- `upstream` remote configured (`patrickdappollonio/dux`).
- audit01 plan phases either completed or explicitly deferred (audit02
  Phases 11/12/13 cover the still-open audit01 P0s).

## Files to touch
- `docs/plans/audits/audit02/artifacts/` — create.
- `docs/plans/audits/audit02/artifacts/00-baseline-rev.txt` — record HEAD sha + branch.
- `docs/plans/audits/audit02/artifacts/00-spot-check.md` — verification log.
- `docs/plans/audits/audit02/artifacts/00-upstream-drift.txt` — `git log HEAD..upstream/main`.
- (no source code changes in this phase)

## Steps
1. **Record baseline**:
   ```bash
   git rev-parse HEAD > docs/plans/audits/audit02/artifacts/00-baseline-rev.txt
   git rev-parse --abbrev-ref HEAD >> docs/plans/audits/audit02/artifacts/00-baseline-rev.txt
   ```

2. **Spot-check audit02 P0 findings** against current HEAD. For each item
   below, run the listed command and confirm the finding still applies.
   If it doesn't (a fix landed in the meantime), strike it from your phase
   list — do not re-fix.

   | ID    | Verify with                                                              | Expected |
   |-------|--------------------------------------------------------------------------|----------|
   | P0-A  | `sed -n '79,85p' dux-amq/wrappers/claude-amq`                            | sees `CLAUDE_AMQ_SAFE` opt-out |
   | P0-A  | `sed -n '27p' dux-amq/wrappers/codex-amq`                                | unconditional `--dangerously-bypass-...` |
   | P0-B  | `sed -n '77,94p' src/logger.rs`                                          | no sanitization in `log()` |
   | P0-D  | `grep -n 'git::is_git_repo\|git::current_branch\|git::changed_files' src/app/sessions.rs src/app/mod.rs` | sync calls on UI thread |
   | P0-E  | `grep -n 'expect("terminal mutex poisoned")' src/pty.rs`                 | 3 hits |
   | P0-F  | `grep -n 'amq init.*--force' dux-amq/install.sh`                         | unconditional `:148` |
   | P0-G  | `sed -n '83,90p' dux-amq/install.sh`                                     | md branch sets `s=1` and never resets |
   | P0-H  | `grep -nE 'uses: [^@]+@(v[0-9]|stable|main)' .github/workflows/*.yml`    | tag-pinned actions in release/pr/test |
   | P0-J  | `grep -n 'reset_agent_data\|fn purge' src/cli.rs`                        | no `purge` command, only `reset` |
   | P0-K  | `grep -n 'amq wake.*--inject-mode raw' dux-amq/wrappers/*`               | 3 hits, no envelope auth |

   Record results in `00-spot-check.md` as a table: ID | confirmed (Y/N) | file:line | snippet.

3. **Re-measure upstream drift** (audit02 inherits audit01 P1-5):
   ```bash
   git fetch upstream
   git log HEAD..upstream/main --oneline \
     | tee docs/plans/audits/audit02/artifacts/00-upstream-drift.txt
   ```

4. **Snapshot CI green**:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   make overlay-test  # shellcheck + bats
   ```
   Resolve any new failure before proceeding. If CI is red on `main`, fix
   that as `chore(audit02): fix pre-existing baseline lint failures` —
   do not start audit02 work on a red baseline.

5. **Verify `cargo audit` and `cargo deny` are installable** (Phase 07
   depends on these). Don't add to CI yet — just confirm they install:
   ```bash
   cargo install --locked cargo-audit cargo-deny
   cargo audit
   cargo deny check 2>&1 | head -20  # may fail without deny.toml; that's fine
   ```

6. **Open PR** `chore(audit02): preflight scaffolding & baseline` containing
   only the artifacts dir. Do NOT include source changes.

## Validation
- `gh pr checks` green on the preflight PR.
- All spot-check rows in `00-spot-check.md` are filled in.
- `00-upstream-drift.txt` has a recorded commit count.

## Acceptance criteria
- [x] `00-baseline-rev.txt` records HEAD sha + branch.
- [x] `00-spot-check.md` verifies all 9 P0 findings.
- [x] `00-upstream-drift.txt` records upstream drift count.
- [x] `cargo fmt`, `clippy -D warnings`, `cargo test`, `make overlay-test` green.
- [x] `cargo audit` and `cargo deny` install without error (verified locally,
  not yet in CI).
- [x] Preflight PR merged — folded into the `audit02/integration` rollup that landed as PR #2.

## Known pitfalls
- audit02 lines reference `554255d`. If `git rev-parse HEAD` is far from
  that commit, lines have probably shifted. Use `grep -n` patterns above
  rather than literal line numbers.
- Don't try to batch fixes into the preflight PR. Keep it artifact-only.

## References
- audit02: `docs/audits/audit02.md`
- audit01 preflight precedent: `docs/plans/audits/audit01/00-preflight.md`
