# Phase 13: TIOCSTI mitigation — detect kernel state, `--inject-via` fallback

> Maps to: **audit01 P1-1** (CONFIRMED CRITICAL by audit02 — AMQ source uses `unix.TIOCSTI` directly with no PTY-master fallback).

## Goal
On stock Ubuntu 24.04 / Debian 12+ kernels (CONFIG_LEGACY_TIOCSTI=n by
default), `amq wake --inject-mode raw` silently fails — every received
message is dropped before it reaches Claude. Detect the kernel state at
install time, document the operator workaround, and provide a working
fallback (e.g. `--inject-via tmux send-keys` or our own bridge).

## Pre-conditions
- Phase 00 baseline green.
- Phase 08 (HMAC envelope) merged — uses the same `--inject-via` plumbing.

## Files to touch
- `dux-amq/install.sh` — add `tiocsti_status()` check, warn the user.
- `dux-amq/wrappers/{claude,codex,gemini}-amq` — wire `--inject-via` if
  `dev.tty.legacy_tiocsti=0`.
- `dux-amq/scripts/dux-amq-inject-bridge` — NEW (a thin tmux-send-keys
  / pts-master-write helper).
- `dux-amq/README.md` — kernel compatibility note.
- (Optional, longer term) Open issue / PR upstream
  `avivsinai/agent-message-queue` to add `pts-master` transport.

## Background
- Linux 6.2 (Nov 2022) made `CONFIG_LEGACY_TIOCSTI` default-off.
- Ubuntu 24.04 LTS, Debian 12+ ship kernels with it disabled.
- Runtime toggle: `sudo sysctl -w dev.tty.legacy_tiocsti=1` (only works
  if the option is compiled in; many distros build it out entirely).
- AMQ source confirmed: `internal/cli/wake_tiocsti_unix.go` calls
  `unix.Syscall(SYS_IOCTL, fd, unix.TIOCSTI, ...)` with no fallback.
- AMQ has `--inject-via <executable>` — runs an external program for
  each notification. Documented as bypassing the TTY requirement.

## Steps

### 13.1 — Detect kernel state at install
`dux-amq/install.sh` after preflight:
```bash
tiocsti_status() {
  # Returns 0 if TIOCSTI usable, 1 if disabled (sysctl=0), 2 if not present.
  local val
  val=$(sysctl -n dev.tty.legacy_tiocsti 2>/dev/null || echo "")
  case "$val" in
    1) return 0 ;;
    0) return 1 ;;
    *) return 2 ;;  # Sysctl key missing → kernel built without it
  esac
}
if ! tiocsti_status; then
  case $? in
    1) warn "kernel: dev.tty.legacy_tiocsti=0 — amq wake injection will not work."
       warn "  Either run 'sudo sysctl -w dev.tty.legacy_tiocsti=1' (if your kernel"
       warn "  supports it) or use --inject-via mode (set DUX_AMQ_INJECT_MODE=via)." ;;
    2) warn "kernel: TIOCSTI not present — using --inject-via mode by default." ;;
  esac
  printf 'tiocsti_disabled' > "$STATE_ROOT/dux/.tiocsti-state"
else
  rm -f "$STATE_ROOT/dux/.tiocsti-state"
fi
```

### 13.2 — Wrappers detect and fall back
Replace the `amq wake … --inject-mode raw` line in each wrapper:
```bash
INJECT_ARGS=(--inject-mode raw)
INJECT_MODE="${DUX_AMQ_INJECT_MODE:-}"
if [[ -z "$INJECT_MODE" ]]; then
  if [[ -f "$STATE_ROOT/dux/.tiocsti-state" ]]; then
    INJECT_MODE="via"
  else
    INJECT_MODE="raw"
  fi
fi
case "$INJECT_MODE" in
  raw)
    INJECT_ARGS=(--inject-mode raw) ;;
  via)
    # Audit02 Phase 13: TIOCSTI disabled — bridge instead.
    INJECT_ARGS=(--inject-via "$LOCAL_BIN/dux-amq-inject-bridge") ;;
  *) warn "DUX_AMQ_INJECT_MODE=$INJECT_MODE not understood; using raw"
     INJECT_ARGS=(--inject-mode raw) ;;
esac

amq wake --me "$ME" --root "$ROOT" "${INJECT_ARGS[@]}" </dev/tty \
  2>"$HOME/.local/share/dux-amq/wake-$ME.log" &
```

### 13.3 — The bridge script
`dux-amq/scripts/dux-amq-inject-bridge`:
```bash
#!/usr/bin/env bash
# Receive a verified payload on stdin; inject it into the dux pane PTY.
# Strategy 1: if running inside tmux, use `tmux send-keys`.
# Strategy 2: write to /proc/$DUX_PANE_PID/fd/0 (requires shared user).
# Strategy 3: write to a file the dux pane is tail'ing (last resort).
set -euo pipefail

read -r body || exit 0  # nothing to inject

if [[ -n "${TMUX:-}" ]]; then
  TARGET="${DUX_TMUX_TARGET:-:.0}"   # current pane
  tmux send-keys -t "$TARGET" -- "$body" Enter
  exit 0
fi

# Fallback: write to dux's own PTY master (requires DUX_PANE_FD env set
# by dux when spawning the wake daemon — TODO upstream patch).
if [[ -n "${DUX_PANE_FD:-}" && -w "/proc/$$/fd/$DUX_PANE_FD" ]]; then
  printf '%s\n' "$body" > "/proc/$$/fd/$DUX_PANE_FD"
  exit 0
fi

# Last resort: file-based queue dux polls.
mkdir -p "$HOME/.local/share/dux-amq/inject-queue"
printf '%s\n' "$body" > "$HOME/.local/share/dux-amq/inject-queue/$(date +%s%N).msg"
```
Strategies 2/3 require dux changes; Strategy 1 (tmux) is good enough
for the common case (most users run dux inside tmux on the spot VM).

### 13.4 — Coordinate with Phase 08
The HMAC envelope (`amq-receive-verify`) writes the cleaned body to its
stdout. AMQ's `--inject-via` then either uses that output as the text
to inject (if AMQ supports chaining) OR we wrap them:
```bash
amq wake … --inject-via 'amq-receive-verify | dux-amq-inject-bridge'
```
Verify against the pinned AMQ version's `--help`.

### 13.5 — Document operator-side
`dux-amq/README.md` — add:
```
## Kernel compatibility (TIOCSTI)

`amq wake` uses TIOCSTI by default. On Linux 6.2+ this is disabled
(CONFIG_LEGACY_TIOCSTI=n) and on Ubuntu 24.04 / Debian 12 ships built
out entirely. The install script detects this and switches to
`--inject-via` mode automatically. Override with:
  DUX_AMQ_INJECT_MODE=raw   # force TIOCSTI even if disabled
  DUX_AMQ_INJECT_MODE=via   # force the bridge

Check at runtime: cat $STATE_ROOT/dux/.tiocsti-state (absent = TIOCSTI ok).
```

### 13.6 — Upstream issue
File `avivsinai/agent-message-queue#NEW`: "Add pts-master transport for
TIOCSTI-disabled kernels". Reference Linux 6.2 deprecation, Ubuntu LP
#2046192. Track in `artifacts/13-upstream-issue.txt`.

## Validation
- Smoke test on Ubuntu 24.04: `sysctl dev.tty.legacy_tiocsti` → 0;
  install runs, wrapper picks `via`, message round-trip works through
  the tmux bridge.
- Smoke test on Ubuntu 22.04 (older): TIOCSTI works, raw mode used.
- `make overlay-test` green.

## Acceptance criteria
- [x] `install.sh` detects kernel state and writes `.tiocsti-state` flag.
- [x] Wrappers branch on `DUX_AMQ_INJECT_MODE` / `.tiocsti-state`.
- [x] `dux-amq-inject-bridge` script implements at least the tmux
      strategy with a documented fallback (file-based queue under `~/.local/share/dux-amq/inject-queue/`).
- [x] `wake-$ME.log` captures stderr instead of `>/dev/null`.
- [x] README "Kernel compatibility" section (`dux-amq/README.md`).
- [x] Upstream issue note recorded in `artifacts/13-upstream-issue.txt` (deferred filing — same constraint as Phase 08).
- [x] PR: `feat(wake): TIOCSTI-disabled fallback via inject bridge (audit01 P1-1)` — landed via PR #2.

## Known pitfalls
- AMQ may not support shell pipelines as `--inject-via` argument
  (single executable expected). If so, write a wrapper script that
  internally calls verify → bridge.
- tmux strategy requires the dux pane to be inside tmux. dux-amq users
  typically run inside tmux already (per README) but verify.
- `DUX_PANE_FD` does not exist today; Strategy 2 is aspirational and
  needs dux-side support. File a follow-up if Strategy 1 isn't enough.
- `sysctl -w dev.tty.legacy_tiocsti=1` requires the option to be
  compiled in. On many cloud VMs it is built out — sysctl returns
  "unknown key" (status 2 in our detector). Document.

## References
- audit01 P1-1; audit02 confirmation in P1-1 row.
- AMQ source `internal/cli/wake_tiocsti_unix.go`.
- Linux kernel CONFIG_LEGACY_TIOCSTI documentation.
- Ubuntu LP #2046192.
