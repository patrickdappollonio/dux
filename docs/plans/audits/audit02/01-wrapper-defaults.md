# Phase 01: Wrapper YOLO defaults + seed default flip

> Maps to: **P0-A** (audit02), **audit01 P0-3** (still open).

## Goal
Flip three unsafe defaults in the dux-amq wrappers from opt-out to opt-in:
1. Claude `--dangerously-skip-permissions` (currently default-on via opt-out
   `CLAUDE_AMQ_SAFE=1`).
2. Codex `--dangerously-bypass-approvals-and-sandbox` (currently default-on
   with **no flag at all**).
3. Claude session-history seeding from parent worktree (currently default-on
   via opt-out `CLAUDE_AMQ_NO_SEED=1`, but the file's own header comment
   says "OFF by default" — self-contradicting).

## Pre-conditions
- Phase 00 baseline green.

## Files to touch
- `dux-amq/wrappers/claude-amq` — flip both flags.
- `dux-amq/wrappers/codex-amq` — add YOLO opt-in.
- `dux-amq/wrappers/gemini-amq` — symmetric review (no YOLO flag today; confirm).
- `dux-amq/README.md` — document the new env-var contract.
- `dux-amq/config/claude-md-additions.md` — note the flag if it surfaces.
- `dux-amq/tests/wrappers.bats` — add fixtures (NEW file).

## Background (do not skip)
Anthropic Claude Code 2025–26 CVEs (CVE-2025-59536, CVE-2026-21852,
CVE-2026-25723, CVE-2026-33068, CVE-2026-35020/35021/35022) all exploited
credential exfil through prompt-injected paths. The single biggest
mitigation is **default-deny** on tool execution. The current overlay
inverts the safe default for every dux pane — by far the largest attack
surface in audit02.

## Steps
1. **`claude-amq` — flip YOLO**. Replace lines 79-85 with:
   ```bash
   # YOLO is OPT-IN. Set CLAUDE_AMQ_YOLO=1 (or legacy CLAUDE_YOLO=1) to enable.
   # Anthropic CVEs in 2025–26 (CVE-2025-59536 et al) targeted credential
   # exfil through this attack class — default-deny is mandatory in prod.
   EXTRA=()
   if [[ "${CLAUDE_AMQ_YOLO:-${CLAUDE_YOLO:-}}" == "1" ]]; then
     EXTRA+=(--dangerously-skip-permissions)
     printf 'claude-amq: YOLO enabled — tool calls will not prompt\n' >&2
   fi
   ```

2. **`claude-amq` — flip seed default + fix self-contradicting header**.
   Replace the header at lines 10-16 and the function gate at line 27:
   ```bash
   # OFF by default. Set CLAUDE_AMQ_SEED_FROM_PARENT=1 to enable. Be aware:
   # rsync clones the parent worktree's full Claude history (~100 MB on heavy
   # repos) — disk amplification N×, possible token-billing escalation, and
   # cross-worktree info leak (parent transcripts may carry secrets/PII from
   # a different feature). Pair with `resume_args = ["--resume"]` if used.
   ```
   ```bash
   seed_session_history() {
     [[ "${CLAUDE_AMQ_SEED_FROM_PARENT:-}" == "1" ]] || return 0
     # … rest unchanged …
   }
   ```

3. **`codex-amq` — symmetric YOLO opt-in**. Replace line 27 with:
   ```bash
   CODEX_EXTRA=()
   if [[ "${CODEX_AMQ_YOLO:-${CLAUDE_YOLO:-}}" == "1" ]]; then
     CODEX_EXTRA+=(--dangerously-bypass-approvals-and-sandbox)
     printf 'codex-amq: YOLO enabled — sandbox bypass active\n' >&2
   fi
   exec amq coop exec --no-wake --no-init --root "$ROOT" --me "$ME" codex -- "${CODEX_EXTRA[@]}" "$@"
   ```

4. **`gemini-amq`** — read the file; confirm no equivalent flag is
   default-on. If found, add the same opt-in pattern with `GEMINI_AMQ_YOLO`.

5. **`dux-amq/README.md`** — add a "Permission model" section with a
   table:
   | Pane     | Env var to enable YOLO              | What it does                                     |
   |----------|-------------------------------------|--------------------------------------------------|
   | claude   | `CLAUDE_AMQ_YOLO=1`                 | passes `--dangerously-skip-permissions`          |
   | codex    | `CODEX_AMQ_YOLO=1`                  | passes `--dangerously-bypass-approvals-and-sandbox` |
   | (legacy) | `CLAUDE_YOLO=1`                     | enables BOTH for backwards compat                |

   Add a "Session seeding" section explaining the disk/billing/leak
   trade-off and the new `CLAUDE_AMQ_SEED_FROM_PARENT=1` opt-in.

6. **`dux-amq/tests/wrappers.bats`** — add fixtures that verify:
   ```bash
   @test "claude-amq does not pass YOLO flag by default" {
     # Run wrapper with a fake `claude` that records argv; assert
     # --dangerously-skip-permissions absent.
   }
   @test "claude-amq passes YOLO flag when CLAUDE_AMQ_YOLO=1" {
     CLAUDE_AMQ_YOLO=1 …  # assert flag present
   }
   @test "codex-amq does not pass sandbox-bypass flag by default" { ... }
   @test "codex-amq passes flag when CODEX_AMQ_YOLO=1" { ... }
   @test "claude-amq does NOT seed by default" { ... }
   @test "claude-amq seeds when CLAUDE_AMQ_SEED_FROM_PARENT=1" { ... }
   ```
   Use the `dux-amq/tests/fakes/` fake-binary harness from audit01.

## Validation
- `make overlay-test` (shellcheck + bats) green.
- Spot-test on a live VM: `claude-amq --print 'hi'` should oneshot pass
  through (existing oneshot bypass at `:21`); interactive should NOT
  show the YOLO banner unless env var is set.
- `git diff` does not touch any line outside the three wrappers + README + bats file.

## Acceptance criteria
- [x] `claude-amq` line ~84 reads `if [[ "${CLAUDE_AMQ_YOLO:-${CLAUDE_YOLO:-}}" == "1" ]]` (was estimated at ~83; +1 line shift after the YOLO-banner `printf` was added inside the gate).
- [x] `claude-amq` line ~28 reads `[[ "${CLAUDE_AMQ_SEED_FROM_PARENT:-}" == "1" ]] || return 0` (was estimated at ~27; +1 line shift after the seed header comment grew from 7 → 8 lines).
- [x] `claude-amq` header comment (line ~10-17) matches the implementation (OFF by default).
- [x] `codex-amq` line ~32 wraps the bypass flag in a `CODEX_AMQ_YOLO` gate; `exec` line moved to ~37 (was estimated at ~27 — the gate block was inserted between the existing `amq wake` background spawn and the `exec`, pushing the `exec` down by ~10 lines).
- [x] README "Permission model" + "Session seeding" sections present.
- [x] `wrappers.bats` covers all 6 cases above (plus 3 extra: legacy `CLAUDE_YOLO` for both panes, and the `CLAUDE_AMQ_SAFE` deprecation warning); passes locally via `make overlay-test`.
- [x] PR opened: `feat(wrappers): default-deny on YOLO + opt-in session seeding` — folded into the `audit02/integration` rollup that landed as PR #2.

## Known pitfalls
- Bash arrays + `set -u` + `"${ARR[@]}"` on empty arrays: macOS Bash 3.2
  errors. Fix is to expand with `${ARR[@]+"${ARR[@]}"}`. Use this pattern
  for both `EXTRA` and `CODEX_EXTRA`.
- Don't add flags to `gemini-amq` speculatively — Gemini CLI doesn't
  expose a comparable `--dangerously-*` flag today.
- Existing users who `export CLAUDE_AMQ_SAFE=1` will be confused after the
  flip. Add a transitional warning: if `CLAUDE_AMQ_SAFE` is set, print a
  one-line stderr note "CLAUDE_AMQ_SAFE is deprecated; YOLO is now opt-in
  via CLAUDE_AMQ_YOLO" and otherwise ignore it.

## References
- Anthropic security advisories — CVE-2025-59536, CVE-2026-21852, CVE-2026-25723.
- Check Point: RCE & API Token Exfiltration via Claude Code project files.
- audit02 P0-A; audit01 P0-1 (revised) and P0-3.
