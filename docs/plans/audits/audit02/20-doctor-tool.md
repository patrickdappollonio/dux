# Phase 20: `dux-amq doctor` triage tool

> Maps to: **audit01 P2-11** (still open). Critical for production operability.

## Goal
A single command operators run when something's wrong, producing a
diagnostic dump that's safe to share. Without it, every other
production-readiness improvement is invisible to operators.

## Pre-conditions
- Phase 09 (`tracing` JSON logs) merged — doctor reads structured fields.
- Phase 02 (install.sh idempotency) merged — doctor verifies `binary.sha256`.

## Files to touch
- `dux-amq/scripts/dux-amq-doctor` — NEW (bash, the bulk).
- `src/cli.rs` — add `dux doctor` subcommand that wraps the bash script
  + adds Rust-side diagnostics.
- `dux-amq/install.sh` — install the doctor script to `$LOCAL_BIN`.
- `dux-amq/tests/doctor.bats` — basic smoke tests.

## Output structure
```
dux-amq doctor [--json] [--anonymize]

== Versions ==
dux:    v0.4.0
amq:    v0.34.0
claude: 2.1.53
codex:  ...
gemini: ...
overlay: v0.1.0

== Binary integrity ==
amq:        SHA matches /data/state/amq/binary.sha256 ✓
.tiocsti-state: ABSENT (TIOCSTI ok)  |  PRESENT (using --inject-via)

== Persistent disk ==
mount:  /data (ext4)
total:  100G
used:   45G (45%)
top dirs:
  /data/state/dux/worktrees: 12G
  /data/state/claude:         8G
  /data/state/amq:           50M

== AMQ ==
queue root: /data/state/amq
agents:     claude, codex, gemini, alice, bob   (5)
inbox depth (oldest message age):
  claude:  3 (47s)
  alice:   0
  bob:    12 (2h)   <-- WARN

== Symlinks ==
~/.claude          → /data/state/claude          ✓
~/.agents          → /data/state/agents          ✓
~/.codex           → /data/state/codex           ✓

== Kernel ==
release: 6.8.0-1019-gcp
dev.tty.legacy_tiocsti: 0    <-- TIOCSTI disabled, --inject-via in use

== Sessions DB ==
path:        /data/state/dux/sessions.sqlite3
journal:     wal
integrity:   ok
backup:      /data/state/dux/sessions.sqlite3.bak (4 min ago)
sessions:    37 active, 12 detached, 84 exited
orphaned:    2 sessions whose worktree path is missing

== Runtime ==
dux pid:     12345 (uptime 3h12m, RSS 180 MB)
claude PIDs: 4
codex PIDs:  1

== Recent errors (last 24h, top 5 from dux.log) ==
2026-05-03T08:14:00Z ERROR dux::workers spawn failed: …
...
```

## Steps

### 20.1 — Bash skeleton
`dux-amq/scripts/dux-amq-doctor`:
```bash
#!/usr/bin/env bash
set -euo pipefail
JSON=0; ANON=0
while (( $# )); do
  case "$1" in
    --json) JSON=1; shift;;
    --anonymize) ANON=1; shift;;
    -h|--help) echo "usage: dux-amq doctor [--json] [--anonymize]"; exit 0;;
    *) shift;;
  esac
done

STATE_ROOT="${STATE_ROOT:-/data/state}"
section() { printf '\n== %s ==\n' "$*"; }
kv()      { printf '%-12s %s\n' "$1:" "$2"; }
warn()    { printf '\033[33m%s\033[0m\n' "$*"; }
ok()      { printf '\033[32m%s\033[0m\n' "$*"; }
bad()     { printf '\033[31m%s\033[0m\n' "$*"; }

# Sections: implement each as a function; main() calls in order.
versions() { ... }
binary_integrity() { ... }
persistent_disk()  { ... }
amq_status()       { ... }
symlinks()         { ... }
kernel()           { ... }
sessions_db()      { ... }
runtime()          { ... }
recent_errors()    { ... }

main() {
  versions; binary_integrity; persistent_disk; amq_status; symlinks;
  kernel; sessions_db; runtime; recent_errors
}
main
```

### 20.2 — Anonymize mode
When `--anonymize`:
- Replace branch names with `branch-N` (numbered in iteration order).
- Replace user paths (`/home/<user>/`, `/data/state/dux/worktrees/<branch>/`)
  with `/HOME/`, `/WT/<branch-N>/`.
- Replace agent IDs from AMQ queue with `agent-N`.
- Replace dux.log error messages' embedded paths the same way.
Useful for shareable diagnostics.

### 20.3 — JSON mode
When `--json`, emit a single JSON object with the same fields. Pipe
to `jq` for parsing in support tools. Use `jq -n --arg ... '{...}'`
inside bash, or shell out to a small Rust subcommand.

### 20.4 — Rust-side wrapper
`src/cli.rs`:
```rust
#[derive(Subcommand)]
pub enum DiagnosticsCmd {
    /// Run the dux-amq-doctor triage script and add Rust-side diagnostics.
    Doctor {
        #[arg(long)] json: bool,
        #[arg(long)] anonymize: bool,
    },
}
```
Implementation: run the bash script (`Command::new("dux-amq-doctor")`)
and append a "Sessions DB" section computed from the live `Storage`
handle (sqlite integrity check, session counts).

### 20.5 — Tests
`dux-amq/tests/doctor.bats`:
```bash
@test "doctor produces all expected sections" {
  setup_isolated_home; ./dux-amq/install.sh
  run dux-amq/scripts/dux-amq-doctor
  [ "$status" -eq 0 ]
  [[ "$output" == *"== Versions =="* ]]
  [[ "$output" == *"== Binary integrity =="* ]]
  [[ "$output" == *"== Persistent disk =="* ]]
  [[ "$output" == *"== AMQ =="* ]]
  [[ "$output" == *"== Kernel =="* ]]
}
@test "doctor --anonymize redacts branch names" { ... }
@test "doctor --json emits valid JSON" {
  run bash -c "dux-amq/scripts/dux-amq-doctor --json | jq ."
  [ "$status" -eq 0 ]
}
@test "doctor exits 0 even when amq queue is uninitialized" { ... }
```

## Validation
- `make overlay-test` green.
- Manual on test VM: `dux-amq doctor` runs without errors, sections
  populate, color codes show on TTY but strip on pipes.
- `dux-amq doctor --json | jq .` parses cleanly.
- `dux-amq doctor --anonymize` shows no `/home/<actual-user>` path.

## Acceptance criteria
- [ ] `dux-amq-doctor` script exists; installed to `$LOCAL_BIN`.
- [ ] All 9 sections render correctly on a healthy system.
- [ ] `--json` output is valid JSON.
- [ ] `--anonymize` redacts paths + branch names + agent IDs.
- [ ] `dux doctor` Rust subcommand wraps + adds sqlite integrity.
- [ ] 4 bats tests pass.
- [ ] README "Operating dux-amq" section references `dux-amq doctor`.
- [ ] PR: `feat(observability): dux-amq doctor triage tool (audit01 P2-11)`.

## Known pitfalls
- Doctor must NEVER hang. Wrap each section in a 5 s timeout
  (`timeout 5 sysctl ...`); print "(timeout)" if exceeded.
- Don't read `dux.log` synchronously if it's huge — last 1000 lines
  is plenty (`tail -n 1000`).
- AMQ queue depth iterates files in `/data/state/amq/<agent>/inbox/`;
  if the queue is enormous, this can be slow. Cap at 100 inbox files
  per agent ("100+ messages") and document.
- Doctor's environment (`STATE_ROOT`, `AMQ_GLOBAL_ROOT`) must match
  install.sh's. Source `bashrc-additions.sh` if present, fall back
  to defaults.
- The Rust-side wrapper inherits `$STATE_ROOT` from the launching
  shell; avoid hard-coding `/data/state`.

## References
- audit01 P2-11.
- audit01 plan precedent: `docs/plans/audits/audit01/16-doctor-tool.md`.
