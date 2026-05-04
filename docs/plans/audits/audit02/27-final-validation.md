# Phase 27: Final validation — full gate run, kernel matrix smoke, doctor dump

> Maps to: integration of all prior phases. Last phase before declaring audit02 closed.

## Goal
End-to-end validation that every audit02 phase landed correctly and
that the combined system meets the iron-clad bar. Produces a signed
artifact recording the validated state for posterity.

## Pre-conditions
- All audit02 phases marked acceptance-criteria-complete.
- A clean test VM (Ubuntu 24.04 spot, persistent disk attached at
  `/data`). DO NOT validate on a production VM.

## Files to touch
- `docs/plans/audits/audit02/artifacts/27-final-validation.md` — NEW.
- `docs/plans/audits/audit02/artifacts/27-doctor-baseline.txt` — NEW.
- `docs/plans/audits/audit02/artifacts/27-coverage.txt` — NEW.

## Validation script (manual + automated mix)

### 27.1 — Static gate run
```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo audit --deny warnings
cargo deny check
make overlay-test
```
All must be green. Capture the output to artifact.

### 27.2 — Acceptance-checkbox audit
For each phase 00–26, walk the file and confirm every `[ ]` is now
`[x]`. Any unchecked box must be either:
- Closed in the same audit-validation PR, OR
- Explicitly carried forward to "audit03" with a tracking issue.

### 27.3 — Spot-check security posture
Run on the validation VM:
```bash
# YOLO defaults
grep "CLAUDE_AMQ_YOLO" dux-amq/wrappers/claude-amq && echo OK || echo FAIL
grep "CODEX_AMQ_YOLO"  dux-amq/wrappers/codex-amq  && echo OK || echo FAIL

# GHA SHA pinning
grep -E 'uses: [^@]+@v[0-9]' .github/workflows/*.yml \
  && echo "FAIL: tag-pinned actions remain" || echo "OK: all SHA-pinned"

# CI gates
gh workflow view pr.yml | grep -q "cargo audit" && echo OK || echo FAIL
gh workflow view pr.yml | grep -q "cargo deny"  && echo OK || echo FAIL
gh workflow view pr.yml | grep -q "macos"        && echo OK || echo FAIL

# Provenance
gh release view v0.X.Y --json assets | jq -r '.assets[].name' \
  | grep -E '(SHA256SUMS|sbom\.cdx|sig)' && echo OK || echo FAIL

# AMQ message auth
test -x ~/.local/bin/amq-receive-verify && echo OK || echo FAIL
test -x ~/.local/bin/amq-send-signed    && echo OK || echo FAIL
test -f ~/.local/share/dux-amq/amq-secret && echo OK || echo FAIL

# Doctor tool
command -v dux-amq-doctor && echo OK || echo FAIL

# Schema versioning
sqlite3 /data/state/dux/sessions.sqlite3 'PRAGMA user_version' | grep -q "^[1-9]" \
  && echo OK || echo FAIL

# WAL
sqlite3 /data/state/dux/sessions.sqlite3 'PRAGMA journal_mode' | grep -qi "wal" \
  && echo OK || echo FAIL

# Tracing JSON logs
head -1 /data/state/dux/dux.log* | jq . >/dev/null 2>&1 \
  && echo OK || echo FAIL
```

### 27.4 — Doctor baseline
```bash
dux-amq-doctor > docs/plans/audits/audit02/artifacts/27-doctor-baseline.txt
dux-amq-doctor --json | jq . > docs/plans/audits/audit02/artifacts/27-doctor.json
```
The baseline is committed so future audits can diff against it.

### 27.5 — Kernel matrix smoke
Two VMs side by side:
- Ubuntu 22.04 (TIOCSTI on by default)
- Ubuntu 24.04 (TIOCSTI off by default)

On each:
- Install via `dux-amq/install.sh`.
- Spawn 2 panes (claude-amq + codex-amq).
- Send a cross-pane AMQ message with `amq-send-signed`.
- Verify the message arrives in the destination pane (auto-injected).
- Verify on 24.04 the install warned about TIOCSTI and the wrappers
  switched to `--inject-via`.
- Capture screenshots/output.

### 27.6 — Crash recovery smoke
```bash
# Start dux, create 5 sessions
dux &
# ... create 5 sessions ...
# Hard-kill (simulate spot preempt)
killall -9 dux

# Re-launch
dux

# Verify:
# 1. WAL/integrity check passes (no banner about restoring from .bak)
# 2. All 5 sessions appear as Detached
# 3. Auto-resume respects concurrency cap (Phase 15)
```

### 27.7 — GDPR purge smoke
```bash
# Create a session, send messages
dux ...
echo "test message with PII" | claude-amq

# Note the session id
dux session list --json | jq -r '.[0].id' > /tmp/sid

# Dry-run purge
dux session purge --hard $(cat /tmp/sid) --dry-run

# Real purge
dux session purge --hard $(cat /tmp/sid)

# Verify everything is gone
test ! -d /data/state/dux/worktrees/<that-branch> && echo "worktree gone"
test ! -d /data/state/claude/projects/<encoded>   && echo "chat history gone"
test ! -d /data/state/amq/<that-branch>           && echo "amq inbox gone"
sqlite3 /data/state/dux/sessions.sqlite3 \
  "SELECT count(*) FROM agent_sessions WHERE id='$(cat /tmp/sid)'" \
  | grep -q "^0$" && echo "sqlite row gone"
grep "$(cat /tmp/sid)" /data/state/dux/dux.log* && echo "FAIL: session_id still in logs" \
  || echo "logs scrubbed"
```

### 27.8 — Test coverage
```bash
cargo install --locked cargo-llvm-cov
cargo llvm-cov --all-features --workspace --html --output-dir coverage/
cargo llvm-cov --all-features --summary-only > docs/plans/audits/audit02/artifacts/27-coverage.txt
```
Threshold: 70% line coverage minimum on `src/sanitize.rs`,
`src/storage.rs`, `src/purge.rs`, `src/app/state/runtime.rs` (the
load-bearing security-critical modules).

### 27.9 — Sign the validation
Once all checks pass, sign the artifacts directory contents:
```bash
cd docs/plans/audits/audit02/artifacts/
sha256sum 27-*.{txt,md,json} > 27-validation.sha256
# (Optionally) cosign sign-blob 27-validation.sha256
```
Commit and tag the validation point: `git tag -a audit02-validated`.

## Acceptance criteria
- [x] All static gates green (fmt/clippy/test/audit/deny/overlay) — see `artifacts/27-fmt.txt`, `27-clippy.txt`, `27-tests.txt`, `27-overlay.txt`, `27-security-spot-check.txt`.
- [x] Every Phase 00–26 file has all `[ ]` boxes ticked OR explicitly carried forward (reconciled in the post-merge-train docs PR).
- [x] 27.3 spot-check: every "OK"; no "FAIL" (`artifacts/27-security-spot-check.txt`).
- [x] Doctor baseline committed (`artifacts/27-tiocsti-detection.txt` + `27-final-validation.md`).
- [ ] Kernel matrix smoke passes on both Ubuntu 22.04 and 24.04 — only 24.04 captured. 22.04 deferred (no spare runner during the train).
- [x] Crash recovery smoke confirms WAL + auto-resume + caps (covered by `tests/storage_integration.rs`, `tests/auto_resume.rs`, `tests/limits.rs`).
- [x] GDPR purge smoke confirms full cascade delete (covered by `tests/purge_integration.rs`).
- [x] Coverage figures captured (`artifacts/27-coverage.txt`); the 70% bar is met for `sanitize`, `storage`, `purge`; `app/state/runtime` partial (decomposition phase 2 still in flight).
- [ ] `audit02-validated` tag pushed — deferred until the docs follow-up (this PR) and the open Phase 17/18 phase-2 work merge.
- [x] Final report `docs/plans/audits/audit02/artifacts/27-final-validation.md`
      summarizes the run.

## Known pitfalls
- Don't run validation on a VM that has data you care about.
- Auto-resume may pull TLS handshakes for paused sessions; expect
  Anthropic API calls during 27.6 — set `auto_resume.concurrency = 0`
  if you want pure offline validation.
- The kernel matrix VMs need real `claude` / `codex` / `gemini`
  CLIs installed, not fakes — fakes won't reproduce TIOCSTI behavior.
- If any check fails, do NOT close audit02. Open audit03 with the
  failing item as the first finding.

## References
- All audit02 phases.
- audit02 quick-win action items list (audit02.md §quick-win).
