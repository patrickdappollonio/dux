# Phase 02: install.sh idempotency hardening

> Maps to: **P0-F** (`amq init --force` queue wipe), **P0-G** (`strip_block` md content-loss), **P1-A** (PATH ambiguity in sha verify), **N-3** (hash-guard fail-open).

## Goal
Make `dux-amq/install.sh` truly idempotent — re-running it should never
destroy operator state. Today: line `:148` runs `amq init --force`
unconditionally, line `:84-90` strips legacy CLAUDE.md content past EOF
without an end sentinel, line `:165-168` pins whichever `amq` is first on
PATH (not the one we just installed), and `bashrc-additions.sh:17-32`
fails open if the hash record is missing.

## Pre-conditions
- Phase 00 baseline green.
- Independent of Phases 01, 03, 04, 05.

## Files to touch
- `dux-amq/install.sh` — four changes (see Steps).
- `dux-amq/config/bashrc-additions.sh` — fail-closed when binary present
  but record absent.
- `dux-amq/tests/install-idempotency.bats` — new test file.
- `dux-amq/tests/strip-block.bats` — new test file (regression for P0-G).

## Steps

### 2.1 — Gate `amq init --force` (P0-F)
Replace `install.sh:148` with:
```bash
# Audit02 P0-F: don't wipe queue config on re-install. AMQ writes its
# state under $STATE_ROOT/amq; the presence of `agents.json` (or whatever
# AMQ's metadata file is — confirm via `amq init --help` against the pinned
# version) tells us init has already run.
AMQ_INIT_MARKER="$STATE_ROOT/amq/agents.json"
if [[ ! -f "$AMQ_INIT_MARKER" ]]; then
  amq init --root "$STATE_ROOT/amq" --agents claude,codex,gemini --force >/dev/null
  ok "amq queue initialized at $STATE_ROOT/amq"
else
  ok "amq queue already initialized at $STATE_ROOT/amq (skipping init)"
fi
chmod 700 "$STATE_ROOT/amq"
```
**Verify** `agents.json` is the right marker by running `amq init --root /tmp/x --agents a,b,c` once and listing the directory. If AMQ uses a different file (e.g. `.amqrc`), update the marker name.

### 2.2 — Fix `strip_block` md branch (P0-G)
The current `awk` rule for legacy CLAUDE.md migration sets `s=1` on the
heading and never resets — it deletes everything from the heading to EOF.
A user who appended their own `## Notes` after the AMQ stanza loses it.

Two fixes; do **both**:

A. Add an explicit end-of-block sentinel to legacy installs by writing it
   on every install for one release cycle, then teach `strip_block` to
   look for it. Edit the `kind=md` branch:
   ```awk
   /^<!-- >>> dux-amq v[^ ]+ >>> -->$/ {s=1; next}
   /^<!-- <<< dux-amq v[^ ]+ <<< -->$/ {s=0; next}
   # Legacy: explicit end sentinel (added by Phase 02). Only present in
   # installs from versions that wrote it.
   /^<!-- end dux-amq legacy -->$/ {s=0; next}
   # Legacy fallback: heading through next "# " or "## " sibling (NOT EOF).
   /^## Multi-agent environment \(AMQ \+ dux\)$/ {s=1; next}
   s && /^## /                                  {s=0}
   !s
   ```
   The `s && /^## /` line resets `s` when awk hits the next `## ` heading,
   so user-appended sections survive.

B. **Snapshot-then-diff guardrail**: before rewriting `~/.claude/CLAUDE.md`,
   copy the original to `$STATE_ROOT/dux/claude-md.bak` and emit the diff
   summary to stderr. Operators can recover.
   ```bash
   if [[ -s "$HOME/.claude/CLAUDE.md" ]]; then
     install -m 0644 "$HOME/.claude/CLAUDE.md" "$STATE_ROOT/dux/claude-md.$(date +%s).bak"
   fi
   ```

### 2.3 — Verify the right `amq` binary (P1-A)
Replace `install.sh:163-171`:
```bash
AMQ_BIN_DIR="$STATE_ROOT/amq-bin"
AMQ_BIN_PINNED="$AMQ_BIN_DIR/amq"
mkdir -p "$AMQ_BIN_DIR"

# Audit02 P1-A: don't trust `command -v amq` here — PATH order is
# unpredictable. If the install branch above ran, $LOCAL_BIN/amq is the
# binary we just verified against $AMQ_SHA256 (tarball hash). If the
# branch was skipped (binary already present), we must hash-check it
# now before pinning.
AMQ_FRESH_INSTALL_BIN="$LOCAL_BIN/amq"
if [[ -x "$AMQ_FRESH_INSTALL_BIN" ]]; then
  AMQ_BIN_SOURCE="$AMQ_FRESH_INSTALL_BIN"
else
  AMQ_BIN_SOURCE="$(command -v amq)"
  [[ -n "$AMQ_BIN_SOURCE" ]] || { warn "amq not found after install"; exit 1; }
fi
verify_sha256 "$AMQ_BIN_SOURCE" "$AMQ_BINARY_SHA256" "amq binary at $AMQ_BIN_SOURCE"
install -m 0755 "$AMQ_BIN_SOURCE" "$AMQ_BIN_PINNED"
sha256sum "$AMQ_BIN_PINNED" > "$STATE_ROOT/amq/binary.sha256"
chmod 0444 "$STATE_ROOT/amq/binary.sha256"  # was 0644; harden to read-only (P1-E prep)
ok "amq binary pinned at $AMQ_BIN_PINNED ($(awk '{print $1}' "$STATE_ROOT/amq/binary.sha256"))"
```

### 2.4 — Fail-closed hash guard (N-3)
Edit `dux-amq/config/bashrc-additions.sh:17-32`:
```bash
_amq_shell_setup_guarded() {
  local rec="$AMQ_GLOBAL_ROOT/binary.sha256"
  if [[ ! -x "$AMQ_BIN" ]]; then
    return 0  # amq not installed yet; nothing to guard
  fi
  if [[ ! -f "$rec" ]]; then
    # Audit02 N-3: previously a silent return 0 here. An attacker with
    # filesystem access could disable the guard by removing one file.
    printf '\033[31m[dux-amq]\033[0m amq binary present but %s missing — refusing to source shell-setup.\n' "$rec" >&2
    printf '            re-run install.sh (or rm "%s" if intentional).\n' "$AMQ_BIN" >&2
    return 1
  fi
  local expected actual
  expected=$(awk '{print $1}' "$rec")
  actual=$(sha256sum "$AMQ_BIN" | awk '{print $1}')
  if [[ "$expected" != "$actual" ]]; then
    printf '\033[31m[dux-amq]\033[0m amq binary sha mismatch — refusing to source shell-setup.\n' >&2
    return 1
  fi
  eval "$("$AMQ_BIN" shell-setup)"
}
_amq_shell_setup_guarded
```

### 2.5 — Tests
`dux-amq/tests/install-idempotency.bats`:
```bash
@test "second install does not wipe amq queue config" {
  setup_isolated_home
  STATE_ROOT="$TEST_HOME/state" ./dux-amq/install.sh    # first install
  echo '{"custom":"agents"}' > "$TEST_HOME/state/amq/agents.json"
  STATE_ROOT="$TEST_HOME/state" ./dux-amq/install.sh    # second install
  grep -q '"custom":"agents"' "$TEST_HOME/state/amq/agents.json"
}
@test "second install preserves user content below legacy CLAUDE.md heading" {
  setup_isolated_home
  cat > "$HOME/.claude/CLAUDE.md" <<'MD'
## Multi-agent environment (AMQ + dux)
old content from pre-phase-12 install
## My personal notes
DO NOT DELETE
MD
  ./dux-amq/install.sh
  grep -q "DO NOT DELETE" "$HOME/.claude/CLAUDE.md"
}
@test "guard refuses eval if binary.sha256 record is removed" {
  setup_isolated_home
  ./dux-amq/install.sh
  rm -f "$TEST_HOME/state/amq/binary.sha256"
  run bash -c 'source dux-amq/config/bashrc-additions.sh'
  [ "$status" -ne 0 ]
}
```

## Validation
- `make overlay-test` green.
- Manual: run `./dux-amq/install.sh` twice on a clean VM; verify `agents.json` is preserved between runs.
- Manual: `rm $STATE_ROOT/amq/binary.sha256 && bash -i -c true` — expect red banner, expect rc != 0.

## Acceptance criteria
- [ ] `install.sh:148` is gated by `[[ ! -f "$AMQ_INIT_MARKER" ]]`.
- [ ] `strip_block` md branch resets `s=0` on next `## ` heading.
- [ ] Pre-rewrite snapshot of CLAUDE.md saved to `$STATE_ROOT/dux/claude-md.<ts>.bak`.
- [ ] `verify_sha256` uses `$LOCAL_BIN/amq` when fresh install ran.
- [ ] `binary.sha256` written 0444 (read-only).
- [ ] `_amq_shell_setup_guarded` fails closed when record missing but binary present.
- [ ] Three new bats tests pass.
- [ ] PR: `fix(install): idempotency hardening (P0-F/G, P1-A, N-3)`.

## Known pitfalls
- Confirm AMQ's actual init marker file name (run `amq init` in a tmpdir;
  `ls -la`). The plan assumes `agents.json`; if it's `.amqrc` or
  `metadata.json`, update accordingly.
- The `awk` `s=0` reset on `## ` won't help if the user wrote `### `
  subheadings inside the legacy block. Document the limitation in a
  release note: "if you had `### ` subheadings inside the dux-amq stanza,
  re-add them after upgrade."
- `chmod 0444` on `binary.sha256` means re-running install.sh has to
  overwrite a read-only file. The `sha256sum > $rec` redirect overwrites
  fine because the parent dir is writable; but on macOS-restricted FS,
  add `chmod u+w "$rec" 2>/dev/null || true` before the redirect.

## References
- audit02 P0-F, P0-G, P1-A, N-3.
