#!/usr/bin/env bats
#
# audit02 phase 22: wrapper P1 hygiene bundle.
#
# Verifies the five fixes in the bundle:
#   P1-B  setsid + disown + PID file → wake survives parent shell hangup.
#   P1-C  install.sh preflight collects ALL missing tools in one pass.
#   P1-D  claude-amq seed reports rsync warning count when non-zero
#         (previously swallowed errors with `2>/dev/null || true`).
#   P1-E  bashrc-additions.sh refuses to eval shell-setup when the amq
#         binary mtime is newer than the recorded `binary.sha256`.
#   P1-F  wrappers refuse to start when the agent handle is already
#         registered to a different $PWD (identity collision).
#
# Implementation notes:
#   - We use the same `tests/fakes/amq` as wrappers.bats so the wrapper
#     can run end-to-end without hitting the real provider.
#   - For P1-B, the fake `amq wake` exits 0 immediately on its own; we
#     instead test that the wrapper writes a PID file and the file
#     contains a numeric pid. Verifying actual SIGHUP-survival would
#     require a much heavier integration harness (real `amq wake` +
#     real PTY); the cheap proof is "the structural change happened".
#   - For P1-C we drive install.sh with a stripped PATH so EVERY tool
#     is missing, then assert the output reports the full list, not
#     just the first miss.

load 'lib/setup'

WRAPPERS_DIR="$BATS_TEST_DIRNAME/../wrappers"
INSTALL_SH="$BATS_TEST_DIRNAME/../install.sh"
BASHRC_ADDITIONS="$BATS_TEST_DIRNAME/../config/bashrc-additions.sh"

setup() {
  setup_isolated_home
  ARGV_FILE="$TEST_HOME/argv.log"
  : >"$ARGV_FILE"
  export AMQ_FAKE_ARGV_FILE="$ARGV_FILE"
  export AM_ME="p1pane"
  unset CLAUDE_AMQ_YOLO CLAUDE_YOLO CLAUDE_AMQ_SAFE
  unset CODEX_AMQ_YOLO
  unset CLAUDE_AMQ_SEED_FROM_PARENT CLAUDE_AMQ_NO_SEED
  unset DUX_AMQ_INJECT_MODE
  export STATE_ROOT="$TEST_HOME/state"
  mkdir -p "$STATE_ROOT/dux"
  # Pin AMQ_GLOBAL_ROOT under $TEST_HOME so the new collision marker
  # doesn't leak into the host's /data/state/amq.
  export AMQ_GLOBAL_ROOT="$TEST_HOME/amq"
  mkdir -p "$AMQ_GLOBAL_ROOT/agents"
}

teardown() {
  teardown_isolated_home
}

# ---------------------------------------------------------------------------
# P1-B: wake durability — setsid + PID file
# ---------------------------------------------------------------------------

@test "P1-B: claude-amq writes wake-\$ME.pid under \$LOG_DIR" {
  run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  pid_file="$HOME/.local/share/dux-amq/wake-p1pane.pid"
  [ -f "$pid_file" ] || {
    printf 'expected pid file at %s; ls:\n' "$pid_file" >&2
    ls -la "$HOME/.local/share/dux-amq/" >&2 || true
    return 1
  }
  pid=$(cat "$pid_file")
  # Pid file must contain a positive integer.
  [[ "$pid" =~ ^[0-9]+$ ]] || {
    printf 'pid file did not contain numeric pid: %q\n' "$pid" >&2
    return 1
  }
  [ "$pid" -gt 0 ]
}

@test "P1-B: codex-amq writes wake-\$ME.pid under \$LOG_DIR" {
  run "$WRAPPERS_DIR/codex-amq"
  [ "$status" -eq 0 ]
  [ -f "$HOME/.local/share/dux-amq/wake-p1pane.pid" ]
}

@test "P1-B: gemini-amq writes wake-\$ME.pid under \$LOG_DIR" {
  run "$WRAPPERS_DIR/gemini-amq"
  [ "$status" -eq 0 ]
  [ -f "$HOME/.local/share/dux-amq/wake-p1pane.pid" ]
}

# ---------------------------------------------------------------------------
# P1-C: preflight collects ALL missing tools, fails with one list.
# ---------------------------------------------------------------------------

@test "P1-C: install.sh preflight reports all missing tools, not just the first" {
  # Drive install.sh with a PATH that contains *only* the absolute-path
  # bash binary (so the shebang resolves) but no required-tool dir, so
  # `command -v curl` / `command -v jq` / etc. all fail. We invoke bash
  # via its absolute path and stub PATH to a sentinel directory that's
  # confirmed empty.
  local empty_path="$TEST_HOME/empty-path"
  mkdir -p "$empty_path"
  # Use the absolute path to bash so we don't need bash on $PATH.
  local bash_bin
  bash_bin=$(command -v bash)
  # Forward STATE_ROOT so install.sh's preflight (parent-of-STATE_ROOT
  # existence check) doesn't trip on a CI runner that has no `/data`
  # mount. The test isolation in setup() points STATE_ROOT under
  # $TEST_HOME, whose parent always exists.
  run env -i HOME="$HOME" PATH="$empty_path" STATE_ROOT="$STATE_ROOT" "$bash_bin" "$INSTALL_SH"
  [ "$status" -ne 0 ]
  # The new aggregate message lists ALL missing tools on one line:
  [[ "$output" == *"missing required tools:"* ]] || {
    printf 'expected aggregate message; got:\n%s\n' "$output" >&2
    return 1
  }
  # Sanity: at least three of the required tools must appear in the
  # message (curl + jq + openssl proves we collected past the first).
  [[ "$output" == *"curl"* ]]
  [[ "$output" == *"jq"* ]]
  [[ "$output" == *"openssl"* ]]
}

@test "P1-C: install.sh preflight lists realpath and openssl as required" {
  # When realpath/openssl are specifically missing they must still be
  # named in the bail message — Phase 12 (path encoding) and Phase 8
  # (HMAC envelope) hard-depend on them.
  local empty_path="$TEST_HOME/empty-path"
  mkdir -p "$empty_path"
  local bash_bin
  bash_bin=$(command -v bash)
  # Forward STATE_ROOT so install.sh's preflight (parent-of-STATE_ROOT
  # existence check) doesn't trip on a CI runner that has no `/data`
  # mount. The test isolation in setup() points STATE_ROOT under
  # $TEST_HOME, whose parent always exists.
  run env -i HOME="$HOME" PATH="$empty_path" STATE_ROOT="$STATE_ROOT" "$bash_bin" "$INSTALL_SH"
  [ "$status" -ne 0 ]
  [[ "$output" == *"realpath"* ]] || {
    printf 'realpath missing from preflight list:\n%s\n' "$output" >&2
    return 1
  }
  [[ "$output" == *"openssl"* ]] || {
    printf 'openssl missing from preflight list:\n%s\n' "$output" >&2
    return 1
  }
}

# ---------------------------------------------------------------------------
# P1-D: claude-amq seed reports rsync warning count when non-zero.
# ---------------------------------------------------------------------------

setup_parent_and_worktree_with_unreadable_file() {
  if ! command -v git >/dev/null 2>&1; then skip "git not available"; fi
  local repo="$TEST_HOME/parent"
  local wt="$TEST_HOME/child"
  mkdir -p "$repo"
  (
    cd "$repo"
    git -c init.defaultBranch=main init -q
    git config user.email "test@example.com"
    git config user.name  "Test"
    : >file
    git add file
    git -c commit.gpgsign=false commit -q -m init
    git worktree add -q -b feature "$wt" >/dev/null
  )
  ENC_PARENT=$(encode-claude-project-dir "$repo")
  ENC_CHILD=$(encode-claude-project-dir "$wt")
  PARENT_SESS_DIR="$HOME/.claude/projects/$ENC_PARENT"
  CHILD_SESS_DIR="$HOME/.claude/projects/$ENC_CHILD"
  mkdir -p "$PARENT_SESS_DIR"
  echo '{"role":"system","content":"hi"}' >"$PARENT_SESS_DIR/sample.jsonl"
  # Drop a file rsync cannot read so it emits a warning to stderr but
  # still copies the rest. `chmod 000` works for non-root callers; the
  # CI runner runs as a non-root user so this is reliable.
  echo "secret" >"$PARENT_SESS_DIR/unreadable.jsonl"
  chmod 000 "$PARENT_SESS_DIR/unreadable.jsonl"
  CHILD_WT="$wt"
  export CHILD_WT PARENT_SESS_DIR CHILD_SESS_DIR
}

@test "P1-D: claude-amq seed reports warning count when rsync emits warnings" {
  if ! command -v git >/dev/null 2>&1; then skip "git not available"; fi
  if ! command -v rsync >/dev/null 2>&1; then skip "rsync not available"; fi
  if [[ "$(id -u)" == "0" ]]; then skip "root bypasses chmod 000"; fi
  setup_parent_and_worktree_with_unreadable_file
  cd "$CHILD_WT"
  CLAUDE_AMQ_SEED_FROM_PARENT=1 run "$WRAPPERS_DIR/claude-amq"
  # restore mode so teardown can rm -rf cleanly
  chmod 644 "$PARENT_SESS_DIR/unreadable.jsonl" 2>/dev/null || true
  [ "$status" -eq 0 ]
  # The new message must mention "rsync warnings" and the temp log path
  # — proving the wrapper actually captured stderr instead of dropping
  # it to /dev/null. Anything containing "with N rsync warnings" is OK.
  [[ "$output" == *"rsync warnings"* ]] || {
    printf 'expected "rsync warnings" in seed output; got:\n%s\n' "$output" >&2
    return 1
  }
}

@test "P1-D: claude-amq seed reports plain count on clean rsync" {
  if ! command -v git >/dev/null 2>&1; then skip "git not available"; fi
  if ! command -v rsync >/dev/null 2>&1; then skip "rsync not available"; fi
  # Re-use setup from wrappers.bats — same shape, no unreadable file.
  local repo="$TEST_HOME/parent"
  local wt="$TEST_HOME/child"
  mkdir -p "$repo"
  (
    cd "$repo"
    git -c init.defaultBranch=main init -q
    git config user.email "test@example.com"
    git config user.name  "Test"
    : >file
    git add file
    git -c commit.gpgsign=false commit -q -m init
    git worktree add -q -b feature "$wt" >/dev/null
  )
  ENC_PARENT=$(encode-claude-project-dir "$repo")
  ENC_CHILD=$(encode-claude-project-dir "$wt")
  mkdir -p "$HOME/.claude/projects/$ENC_PARENT"
  echo '{"role":"system","content":"hi"}' >"$HOME/.claude/projects/$ENC_PARENT/sample.jsonl"
  cd "$wt"
  CLAUDE_AMQ_SEED_FROM_PARENT=1 run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  # No warnings expected — the original "seeded N past sessions" path
  # must still fire on the clean case.
  [[ "$output" == *"seeded "*" past sessions"* ]] || {
    printf 'expected clean seed message; got:\n%s\n' "$output" >&2
    return 1
  }
  [[ "$output" != *"rsync warnings"* ]] || {
    printf 'unexpected warning text on clean rsync; got:\n%s\n' "$output" >&2
    return 1
  }
}

# ---------------------------------------------------------------------------
# P1-E: hash guard refuses when binary mtime > recorded sha256 mtime.
# ---------------------------------------------------------------------------

@test "P1-E: bashrc guard refuses when amq binary is newer than binary.sha256" {
  # Build a tiny isolated install: a fake `amq` binary, a recorded
  # sha256 file, then `touch` the binary so its mtime is newer.
  local bin_dir="$TEST_HOME/state/amq-bin"
  local rec_dir="$TEST_HOME/state/amq"
  mkdir -p "$bin_dir" "$rec_dir"
  local amq_bin="$bin_dir/amq"
  printf '#!/bin/sh\necho fake\n' >"$amq_bin"
  chmod 0755 "$amq_bin"
  sha256sum "$amq_bin" >"$rec_dir/binary.sha256"
  # Make the binary strictly newer than the record. `touch -t` with a
  # past time on the record is the most portable way; some filesystems
  # don't support sub-second mtimes, so a 60-second gap is required.
  touch -d "@$(($(date +%s) - 120))" "$rec_dir/binary.sha256"
  touch -d "@$(date +%s)"             "$amq_bin"

  run env \
    AMQ_BIN="$amq_bin" \
    AMQ_GLOBAL_ROOT="$rec_dir" \
    bash -c 'set -e; source "'"$BASHRC_ADDITIONS"'"'
  [ "$status" -ne 0 ] || {
    printf 'guard accepted out-of-band binary update. output:\n%s\n' "$output" >&2
    return 1
  }
  [[ "$output" == *"newer than recorded hash"* ]] || {
    printf 'guard fired but did not mention mtime; got:\n%s\n' "$output" >&2
    return 1
  }
}

@test "P1-E: bashrc guard accepts when binary.sha256 mtime > binary mtime" {
  # The complementary case: a fresh install (record written *after* the
  # binary was placed) must NOT trigger the mtime guard. The hash check
  # downstream may still fail in this synthetic setup because we don't
  # actually invoke `amq shell-setup` — but the guard's own mtime path
  # must succeed. We assert the absence of the mtime banner specifically.
  local bin_dir="$TEST_HOME/state/amq-bin"
  local rec_dir="$TEST_HOME/state/amq"
  mkdir -p "$bin_dir" "$rec_dir"
  local amq_bin="$bin_dir/amq"
  printf '#!/bin/sh\necho fake; echo "alias amq=true"\n' >"$amq_bin"
  chmod 0755 "$amq_bin"
  sha256sum "$amq_bin" >"$rec_dir/binary.sha256"
  # Record strictly newer than binary.
  touch -d "@$(($(date +%s) - 120))" "$amq_bin"
  touch -d "@$(date +%s)"             "$rec_dir/binary.sha256"

  run env \
    AMQ_BIN="$amq_bin" \
    AMQ_GLOBAL_ROOT="$rec_dir" \
    bash -c 'source "'"$BASHRC_ADDITIONS"'"'
  # The mtime banner must NOT appear, regardless of whether the
  # downstream hash check or shell-setup eval passes/fails.
  [[ "$output" != *"newer than recorded hash"* ]] || {
    printf 'mtime banner fired when it should not have:\n%s\n' "$output" >&2
    return 1
  }
}

# ---------------------------------------------------------------------------
# P1-F: identity collision detection.
# ---------------------------------------------------------------------------

@test "P1-F: claude-amq refuses when handle already registered to a different \$PWD" {
  # First invocation registers $PWD = $TEST_HOME under the
  # already-pinned AMQ_GLOBAL_ROOT.
  cd "$TEST_HOME"
  run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  marker="$AMQ_GLOBAL_ROOT/agents/p1pane/.dux-amq-source"
  [ -f "$marker" ]
  [[ "$(cat "$marker")" == "$TEST_HOME" ]]

  # Second invocation from a different directory MUST be refused.
  mkdir -p "$TEST_HOME/elsewhere"
  cd "$TEST_HOME/elsewhere"
  run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -ne 0 ] || {
    printf 'expected collision refusal; got status %s output:\n%s\n' \
      "$status" "$output" >&2
    return 1
  }
  [[ "$output" == *"identity collision"* ]] || {
    printf 'expected "identity collision" banner; got:\n%s\n' "$output" >&2
    return 1
  }
  # And the marker must still point at the *first* $PWD — second
  # invocation is refused, not silently overwritten.
  [[ "$(cat "$marker")" == "$TEST_HOME" ]]
}

@test "P1-F: same handle from same \$PWD is allowed (idempotent)" {
  cd "$TEST_HOME"
  run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  # Re-invoke from the same dir — must NOT collide.
  run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ] || {
    printf 'second same-cwd invocation falsely flagged collision:\n%s\n' "$output" >&2
    return 1
  }
}

@test "P1-F: codex-amq also enforces collision detection" {
  cd "$TEST_HOME"
  run "$WRAPPERS_DIR/codex-amq"
  [ "$status" -eq 0 ]
  mkdir -p "$TEST_HOME/elsewhere"
  cd "$TEST_HOME/elsewhere"
  run "$WRAPPERS_DIR/codex-amq"
  [ "$status" -ne 0 ]
  [[ "$output" == *"identity collision"* ]]
}

@test "P1-F: gemini-amq also enforces collision detection" {
  cd "$TEST_HOME"
  run "$WRAPPERS_DIR/gemini-amq"
  [ "$status" -eq 0 ]
  mkdir -p "$TEST_HOME/elsewhere"
  cd "$TEST_HOME/elsewhere"
  run "$WRAPPERS_DIR/gemini-amq"
  [ "$status" -ne 0 ]
  [[ "$output" == *"identity collision"* ]]
}
