# Phase 11: Migration safety — flock + atomic swap + drop `--delete`

> Maps to: **audit01 P0-4** (still open).

## Goal
Make `dux-amq/scripts/finalize-claude-migration.sh` crash-safe and
race-free. Today: a `pgrep` "no claude running" check at the top, then
seconds later `rsync --delete` + `mv` + `ln -s` happen. Concurrent
`claude` spawns or Ctrl-C/OOM/spot-preempt mid-script all corrupt state.
`rsync --delete` also silently destroys any pre-existing persistent-disk
content.

## Pre-conditions
- Phase 00 baseline green.
- Independent of all other phases.

## Files to touch
- `dux-amq/scripts/finalize-claude-migration.sh` — full rewrite.
- `dux-amq/tests/finalize-migration.bats` — NEW (regression tests).

## Background
`mv -T` is atomic on the same filesystem **only when the target is a
file or empty directory** (Linux `rename(2)` semantics — not macOS).
The deploy-via-symlink pattern (replace one symlink with another) is
the only way to atomically swap a populated directory. Sources:
BashFAQ/045, axialcorps "Atomically replacing files and directories".

## Steps

### 11.1 — Acquire flock at script entry
```bash
#!/usr/bin/env bash
set -euo pipefail

LOCK="/tmp/dux-amq-finalize.lock"
exec 9>"$LOCK"
flock -n 9 || { echo "[finalize] another instance is running" >&2; exit 1; }
trap 'rm -f "$LOCK"' EXIT  # release implicit on fd close, this is cosmetic
```

### 11.2 — Re-check `pgrep` immediately before each destructive op
Don't trust the early check. Wrap in a helper and call before each
mutate:
```bash
ensure_no_claude() {
  if pgrep -x claude >/dev/null; then
    echo "[finalize] claude is running — aborting" >&2; exit 1
  fi
}
ensure_no_claude   # at start
# ... preflight checks, sizing, sanity ...
ensure_no_claude   # before rsync
ensure_no_claude   # before swap
```

### 11.3 — Drop `--delete` from rsync
`rsync` should be **additive**: copy missing files into the persistent
target, never delete what's already there. If the operator wants delete
semantics they should pass `--force-delete` explicitly:
```bash
RSYNC_DELETE=()
if [[ "${FINALIZE_FORCE_DELETE:-}" == "1" ]]; then
  RSYNC_DELETE+=(--delete)
  echo "[finalize] FINALIZE_FORCE_DELETE=1 — rsync will delete extra files in $dst" >&2
fi
rsync -aH "${RSYNC_DELETE[@]}" "$src/" "$dst/"
```

### 11.4 — Atomic symlink swap
Stage the new symlink target, then atomic rename. On Linux `rename(2)`
guarantees atomicity for symlinks within the same directory:
```bash
# Stage
NEW_LINK="$HOME/.claude.new"
ln -sfn "$dst" "$NEW_LINK"

ensure_no_claude

# Atomic swap (Linux: rename(2) is atomic between symlinks)
mv -Tn "$NEW_LINK" "$HOME/.claude"
# -T forbid trailing-slash directory rename interpretation
# -n  do not overwrite if target exists; combined with stage we expect it to NOT exist after the next step
```

If `~/.claude` already exists as a directory (first migration), back it
up first to a timestamped path so we never overwrite live data:
```bash
if [[ -d "$HOME/.claude" && ! -L "$HOME/.claude" ]]; then
  ts=$(date +%s)
  mv "$HOME/.claude" "$HOME/.claude.bak.$ts"
  echo "[finalize] backed up old ~/.claude to ~/.claude.bak.$ts" >&2
fi
```

### 11.5 — Tests
`dux-amq/tests/finalize-migration.bats`:
```bash
@test "finalize aborts when claude is running" {
  setup_isolated_home
  # Fake `pgrep` returning success (= claude running).
  printf '#!/bin/sh\nexit 0\n' > "$BATS_TEST_DIRNAME/fakes/pgrep"
  chmod +x "$BATS_TEST_DIRNAME/fakes/pgrep"
  run dux-amq/scripts/finalize-claude-migration.sh
  [ "$status" -ne 0 ]
}
@test "finalize is idempotent — second run is a no-op when state is consistent" { ... }
@test "finalize preserves pre-existing files at destination by default" {
  # Create $STATE_ROOT/claude/preserved.txt; run finalize; assert it survives.
}
@test "finalize honors FINALIZE_FORCE_DELETE=1 if explicitly passed" { ... }
@test "two parallel finalize invocations: one wins, one fails fast" {
  # Background invocation 1, foreground invocation 2 — second exits 1 fast.
}
```

## Validation
- `make overlay-test` green.
- Manual: pre-populate `$STATE_ROOT/claude/foo` with a sentinel file;
  run finalize; verify sentinel survived.
- Manual: `kill -9` the script mid-run (in a tight loop with another
  shell); verify `~/.claude` always points to either the old or new
  target — never missing.

## Acceptance criteria
- [ ] `flock -n 9` at script entry; second invocation fails fast.
- [ ] `ensure_no_claude` called before each destructive op (≥ 3 times).
- [ ] Default rsync invocation has NO `--delete`.
- [ ] Pre-existing `~/.claude` directory backed up to
      `~/.claude.bak.<ts>` (never deleted in-place).
- [ ] Symlink swap uses `mv -Tn` on a staged `~/.claude.new`.
- [ ] 5 bats tests pass.
- [ ] PR: `fix(finalize): flock + atomic swap + drop --delete (audit01 P0-4)`.

## Known pitfalls
- macOS `mv` does NOT call `rename(2)` for directories the same way as
  GNU `mv`. The atomic-symlink-swap pattern (this phase) works on both,
  but only because we're swapping symlinks not directories. Document.
- `flock -n 9` requires `util-linux`'s flock; macOS doesn't ship one
  by default. macOS users would need `brew install flock` OR we provide
  a fallback using `mkdir`-as-lock pattern (BashFAQ/045). The persistent
  disk migration is Linux-only per `dux-amq/README.md`, so flock is OK.
- `ln -sfn` is non-atomic on its own; the atomicity comes from
  staging-then-rename. Don't simplify to `ln -sfn $dst ~/.claude`.
- `pgrep -x claude` matches exact name; if the user runs `claude-amq`
  or some wrapper, that's a different binary — and the user expects
  finalize to detect both. Add `pgrep -x 'claude(-amq)?'` regex match.

## References
- audit01 P0-4.
- BashFAQ/045: "How can I ensure that only one instance of a script is running at a time?"
- axialcorps: "Atomically replacing files and directories" (2013).
- `man 2 rename` — atomicity guarantees.
