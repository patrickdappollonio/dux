#!/usr/bin/env bats
#
# audit02 phase 01: wrapper YOLO defaults + seed default flip.
#
# Verifies that:
#   1. claude-amq does NOT pass --dangerously-skip-permissions by default.
#   2. claude-amq DOES pass it when CLAUDE_AMQ_YOLO=1.
#   3. codex-amq does NOT pass --dangerously-bypass-approvals-and-sandbox by default.
#   4. codex-amq DOES pass it when CODEX_AMQ_YOLO=1.
#   5. claude-amq does NOT seed parent session history by default.
#   6. claude-amq DOES seed when CLAUDE_AMQ_SEED_FROM_PARENT=1.
#
# Implementation: tests/fakes/amq records argv to $AMQ_FAKE_ARGV_FILE
# (and is a no-op for `amq wake` so the background daemon spawn doesn't
# pollute the recording). The wrappers `exec amq coop exec ... claude --
# <flags> <user-args>`, so asserting the presence/absence of a flag in
# the recorded argv is equivalent to asserting what reaches the provider.

load 'lib/setup'

WRAPPERS_DIR="$BATS_TEST_DIRNAME/../wrappers"

setup() {
  setup_isolated_home
  ARGV_FILE="$TEST_HOME/argv.log"
  : >"$ARGV_FILE"
  export AMQ_FAKE_ARGV_FILE="$ARGV_FILE"
  # Pin a stable identity so the wrapper doesn't need git or DUX_HOME.
  export AM_ME="testpane"
  # Force-unset every env knob the wrappers consult so each test starts
  # from a clean default-deny baseline regardless of the host shell.
  unset CLAUDE_AMQ_YOLO CLAUDE_YOLO CLAUDE_AMQ_SAFE
  unset CODEX_AMQ_YOLO
  unset CLAUDE_AMQ_SEED_FROM_PARENT CLAUDE_AMQ_NO_SEED
  # Audit02 Phase 13: pin STATE_ROOT under the throwaway $TEST_HOME so
  # wrappers don't pick up a real /data/state/dux/.tiocsti-state from
  # the host VM and silently flip into bridge mode for these tests.
  export STATE_ROOT="$TEST_HOME/state"
  mkdir -p "$STATE_ROOT/dux"
  unset DUX_AMQ_INJECT_MODE
  # Audit02 Phase 22 (P1-F): pin AMQ_GLOBAL_ROOT under the throwaway
  # $TEST_HOME so the new identity-collision marker doesn't accumulate
  # in the host VM's /data/state/amq/agents/testpane/ directory and make
  # the second test invocation fail with "identity collision".
  export AMQ_GLOBAL_ROOT="$TEST_HOME/amq"
  mkdir -p "$AMQ_GLOBAL_ROOT/agents"
}

teardown() {
  teardown_isolated_home
}

# Helper: assert the recorded argv contains the given flag literal.
assert_argv_contains() {
  local needle="$1"
  if ! grep -Fxq -- "$needle" "$ARGV_FILE"; then
    {
      printf 'expected flag %q in recorded argv but did not find it\n' "$needle"
      printf '--- recorded argv ---\n'
      cat "$ARGV_FILE"
    } >&2
    return 1
  fi
}

# Helper: assert the recorded argv does NOT contain the given flag.
assert_argv_missing() {
  local needle="$1"
  if grep -Fxq -- "$needle" "$ARGV_FILE"; then
    {
      printf 'flag %q must NOT appear in argv but did\n' "$needle"
      printf '--- recorded argv ---\n'
      cat "$ARGV_FILE"
    } >&2
    return 1
  fi
}

@test "claude-amq does not pass YOLO flag by default" {
  run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  assert_argv_missing "--dangerously-skip-permissions"
}

@test "claude-amq passes YOLO flag when CLAUDE_AMQ_YOLO=1" {
  CLAUDE_AMQ_YOLO=1 run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  assert_argv_contains "--dangerously-skip-permissions"
}

@test "claude-amq passes YOLO flag when legacy CLAUDE_YOLO=1" {
  CLAUDE_YOLO=1 run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  assert_argv_contains "--dangerously-skip-permissions"
}

@test "claude-amq prints transitional warning when CLAUDE_AMQ_SAFE is set" {
  CLAUDE_AMQ_SAFE=1 run "$WRAPPERS_DIR/claude-amq"
  [ "$status" -eq 0 ]
  [[ "$output" == *"CLAUDE_AMQ_SAFE is deprecated"* ]] \
    || { printf 'missing deprecation warning. output:\n%s\n' "$output" >&2; return 1; }
  # And CLAUDE_AMQ_SAFE must NOT silently re-enable YOLO:
  assert_argv_missing "--dangerously-skip-permissions"
}

@test "codex-amq does not pass sandbox-bypass flag by default" {
  run "$WRAPPERS_DIR/codex-amq"
  [ "$status" -eq 0 ]
  assert_argv_missing "--dangerously-bypass-approvals-and-sandbox"
}

@test "codex-amq passes sandbox-bypass flag when CODEX_AMQ_YOLO=1" {
  CODEX_AMQ_YOLO=1 run "$WRAPPERS_DIR/codex-amq"
  [ "$status" -eq 0 ]
  assert_argv_contains "--dangerously-bypass-approvals-and-sandbox"
}

@test "codex-amq passes sandbox-bypass flag when legacy CLAUDE_YOLO=1" {
  CLAUDE_YOLO=1 run "$WRAPPERS_DIR/codex-amq"
  [ "$status" -eq 0 ]
  assert_argv_contains "--dangerously-bypass-approvals-and-sandbox"
}

# --- seed tests --------------------------------------------------------------
#
# The seed path needs a real git worktree-with-parent setup (the wrapper
# calls git rev-parse / git worktree list to detect the topology). We
# build a minimal one in $TEST_HOME and cd into the child worktree
# before invoking claude-amq.
#
# The fake `amq` is still on PATH so the actual provider never runs.

setup_parent_and_worktree() {
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
  # Phase 12: encoding is now done by the shared encode-claude-project-dir
  # script (single source of truth). It replaces every non-[A-Za-z0-9-]
  # char with `-`, including `.` (the old inline sed kept `.`). Calling
  # the script here keeps test fixtures in sync with wrapper behavior.
  ENC_PARENT=$(encode-claude-project-dir "$repo")
  ENC_CHILD=$(encode-claude-project-dir "$wt")
  PARENT_SESS_DIR="$HOME/.claude/projects/$ENC_PARENT"
  CHILD_SESS_DIR="$HOME/.claude/projects/$ENC_CHILD"
  mkdir -p "$PARENT_SESS_DIR"
  echo '{"role":"system","content":"hi"}' >"$PARENT_SESS_DIR/sample.jsonl"
  CHILD_WT="$wt"
  export CHILD_WT PARENT_SESS_DIR CHILD_SESS_DIR
}

@test "claude-amq does NOT seed by default" {
  if ! command -v git >/dev/null 2>&1; then skip "git not available"; fi
  setup_parent_and_worktree
  (
    cd "$CHILD_WT"
    "$WRAPPERS_DIR/claude-amq"
  )
  [ ! -e "$CHILD_SESS_DIR/sample.jsonl" ] \
    || { printf 'seed happened without opt-in (file: %s)\n' "$CHILD_SESS_DIR/sample.jsonl" >&2; return 1; }
}

@test "claude-amq seeds when CLAUDE_AMQ_SEED_FROM_PARENT=1" {
  if ! command -v git >/dev/null 2>&1; then skip "git not available"; fi
  setup_parent_and_worktree
  (
    cd "$CHILD_WT"
    CLAUDE_AMQ_SEED_FROM_PARENT=1 "$WRAPPERS_DIR/claude-amq"
  )
  [ -f "$CHILD_SESS_DIR/sample.jsonl" ] \
    || { printf 'seed did not happen (missing: %s)\n' "$CHILD_SESS_DIR/sample.jsonl" >&2; return 1; }
}
