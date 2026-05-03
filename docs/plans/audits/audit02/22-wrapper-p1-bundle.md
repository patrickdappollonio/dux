# Phase 22: Wrapper P1 hygiene bundle

> Maps to: **P1-B** (orphaned wake), **P1-C** (preflight collect), **P1-D** (rsync error capture), **P1-E** (hash guard fail-closed — partly in Phase 02), **P1-F** (identity collision).

## Goal
Five small wrapper-side hygiene fixes that didn't justify their own
phases. Bundle into a single PR.

## Pre-conditions
- Phase 00 baseline green.
- Phase 01 (wrapper YOLO defaults) merged — same files; avoid conflicts.
- Phase 02 (install.sh idempotency) merged — P1-E partly there.

## Files to touch
- `dux-amq/wrappers/{claude,codex,gemini}-amq` — P1-B, P1-D, P1-F.
- `dux-amq/install.sh` — P1-C.
- `dux-amq/config/bashrc-additions.sh` — P1-E final tightening.
- `dux-amq/tests/wrappers-p1.bats` — NEW.

## Steps

### 22.1 — P1-B: setsid + disown + PID file
Replace `amq wake … &` lines in each wrapper:
```bash
LOG_DIR="$HOME/.local/share/dux-amq"
mkdir -p "$LOG_DIR"
WAKE_LOG="$LOG_DIR/wake-$ME.log"
WAKE_PID="$LOG_DIR/wake-$ME.pid"

setsid amq wake --me "$ME" --root "$ROOT" "${INJECT_ARGS[@]}" \
  </dev/tty >>"$WAKE_LOG" 2>&1 &
disown
echo $! > "$WAKE_PID"
```
`setsid` decouples from the controlling TTY; `disown` removes the
job from bash's table; PID file lets the doctor check liveness.

### 22.2 — P1-C: preflight collect
`dux-amq/install.sh` — replace the loop at `:103-108`:
```bash
missing=()
for tool in curl jq sha256sum tar install git rsync awk sed realpath openssl; do
  command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if (( ${#missing[@]} > 0 )); then
  warn "missing required tools: ${missing[*]}"
  warn "  Debian/Ubuntu: apt-get install -y ${missing[*]}"
  warn "  macOS:         brew install ${missing[*]}"
  exit 1
fi
```
Note: list extends with `realpath` (Phase 12) and `openssl` (Phase 8).

### 22.3 — P1-D: rsync error capture in seed
`claude-amq` — replace the rsync line:
```bash
local rsync_log
rsync_log=$(mktemp)
if rsync -a "$main_dir"/ "$self_dir"/ 2>"$rsync_log"; then
  : # ok
else
  rc=$?
  echo "claude-amq: seed partial (rsync rc=$rc); see $rsync_log" >&2
  # Don't fail wrapper — seed is best-effort.
fi
local n
n=$(find "$self_dir" -name '*.jsonl' 2>/dev/null | wc -l)
if [[ -s "$rsync_log" ]]; then
  echo "claude-amq: seeded $n files (with $(wc -l <"$rsync_log") rsync warnings; see $rsync_log)" >&2
else
  echo "claude-amq: seeded $n past sessions from $main_worktree" >&2
  rm -f "$rsync_log"
fi
```

### 22.4 — P1-E final tightening
Phase 02 fixed the silent-no-op fail-open. Now make the recorded
hash file 0444 + ensure install.sh chmods correctly. Also: emit a
warning if the `binary.sha256` file is older than the binary mtime
(suggests the binary was updated out-of-band):
```bash
if [[ -f "$rec" && -x "$AMQ_BIN" ]]; then
  if [[ "$AMQ_BIN" -nt "$rec" ]]; then
    printf '\033[33m[dux-amq]\033[0m amq binary newer than recorded hash — re-run install.sh\n' >&2
    return 1
  fi
fi
```

### 22.5 — P1-F: identity collision detection
In each wrapper, after the `tr | sed` normalization:
```bash
ME=$(printf '%s' "$ME" | tr '[:upper:]' '[:lower:]' | sed 's|[^a-z0-9_-]|-|g; s|^-\+||; s|-\+$||')

# Collision detection: AMQ stores agents under $ROOT/agents/<name>/.
# If the normalized ME corresponds to a different real branch already
# registered with this name, refuse to start to avoid silent overwrite.
ME_REGISTRATION="$ROOT/agents/$ME/.dux-amq-source"
if [[ -f "$ME_REGISTRATION" ]]; then
  prev=$(cat "$ME_REGISTRATION")
  if [[ "$prev" != "$PWD" ]]; then
    printf '\033[31m[dux-amq]\033[0m identity collision: %s already registered to %s (current: %s).\n' \
      "$ME" "$prev" "$PWD" >&2
    printf '            Rename one branch (avoid normalize-collisions) or set AM_ME explicitly.\n' >&2
    exit 1
  fi
fi
mkdir -p "$ROOT/agents/$ME"
echo "$PWD" > "$ME_REGISTRATION"
```
The marker is mode 0644 (informational). Phase 13's TIOCSTI test fixes
do not depend on this; both Phase 22 and Phase 13 can land in either order.

### 22.6 — Tests
`dux-amq/tests/wrappers-p1.bats`:
```bash
@test "wake survives parent shell hangup (setsid)" {
  setup_isolated_home; ./dux-amq/install.sh
  bash -c 'claude-amq --print &  sleep 0.2; kill -HUP $$'
  # wake PID should still be running
  test -f "$HOME/.local/share/dux-amq/wake-*.pid"
  pid=$(cat "$HOME/.local/share/dux-amq/wake-*.pid" | head -1)
  kill -0 "$pid"
}
@test "preflight collects all missing tools" {
  PATH="/nonexistent" run ./dux-amq/install.sh
  [[ "$output" == *"missing required tools:"* ]]
}
@test "rsync error capture reports partial seed count" { ... }
@test "identity collision detection refuses second invocation" { ... }
@test "guard refuses when binary mtime > rec mtime" { ... }
```

## Validation
- `make overlay-test` green.
- Manual: `claude-amq --print 'hi'` then exit; `cat wake-$ME.log` shows
  AMQ wake's stdout/stderr captured.
- Manual: rename a branch to one that normalizes the same as another;
  second wrapper invocation refuses with the red banner.

## Acceptance criteria
- [ ] All three wrappers use `setsid amq wake … & disown; echo $! > $WAKE_PID`.
- [ ] `wake-$ME.log` and `wake-$ME.pid` written under `~/.local/share/dux-amq/`.
- [ ] `install.sh` preflight collects ALL missing tools, fails with one list.
- [ ] `claude-amq` seed reports rsync error count when non-zero.
- [ ] Hash guard checks recorded mtime vs binary mtime.
- [ ] Identity collision detected; refuses to start.
- [ ] 5 bats tests pass.
- [ ] PR: `fix(wrappers): wake durability + preflight + identity collision (P1-B/C/D/E/F)`.

## Known pitfalls
- `setsid` on macOS may differ from Linux util-linux; verify on macOS
  matrix CI (Phase 21).
- Identity collision detection requires `$ROOT/agents/<name>/` to be
  writable — already true if `amq init` ran (Phase 02 gates it).
- The collision marker uses `$PWD` literally (not realpath); intentional
  so users who symlink-mount different locations don't false-positive.

## References
- audit02 P1-B/C/D/E/F.
