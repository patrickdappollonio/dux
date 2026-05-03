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
  # The wrapper's seed encodes the worktree path with `s|/|-|g; s|_|-|g`.
  ENC_PARENT=$(echo "$repo" | sed 's|/|-|g; s|_|-|g')
  ENC_CHILD=$(echo "$wt"   | sed 's|/|-|g; s|_|-|g')
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
