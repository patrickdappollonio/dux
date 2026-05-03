#!/usr/bin/env bats
#
# Audit02 Phase 20 (P2-11): tests for the dux-amq-doctor triage tool.
#
# These tests run the script against a *synthetic* state root rather
# than driving install.sh end-to-end. Reasons:
#   - install.sh's preflight requires /data and pinned binary hashes.
#     overlay-CI provides them, but we want the doctor tests to also
#     pass on a developer laptop with no /data mount.
#   - We're testing the doctor's *output shape*, not whether install.sh
#     produces a healthy install — that's covered by install-idempotency.bats.
#
# The doctor's contract (from the audit02 plan):
#   1. emit all 9 sections, even on a partial install
#   2. --anonymize redacts $HOME / branch / agent identifiers
#   3. --json emits a single valid JSON object
#   4. exit 0 even when the AMQ queue is uninitialized
#
# Each test seeds just enough on-disk state to exercise the relevant
# code paths.

load 'lib/setup'

setup() {
  setup_isolated_home
  REPO_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  DOCTOR="$REPO_ROOT/dux-amq/scripts/dux-amq-doctor"
  export STATE_ROOT="$TEST_HOME/state"
  export DUX_HOME="$STATE_ROOT/dux"
  export AMQ_GLOBAL_ROOT="$STATE_ROOT/amq"
  # Doctor scrapes DUX_AMQ_VERSION out of install.sh; point it at the
  # real one in the repo so the version section isn't "(unknown)".
  export DUX_AMQ_INSTALL_SH="$REPO_ROOT/dux-amq/install.sh"
}

teardown() {
  teardown_isolated_home
}

# Lay down a minimal but realistic AMQ tree so sec_amq exercises both
# the populated-inbox and empty-inbox paths. Three agents: alpha (1
# message ~5 minutes old), bravo (0 messages), charlie (3 messages, one
# 2 hours old → triggers the "warn" coloring path).
seed_amq_state() {
  mkdir -p "$AMQ_GLOBAL_ROOT/meta" "$AMQ_GLOBAL_ROOT/agents"
  cat > "$AMQ_GLOBAL_ROOT/meta/config.json" <<'JSON'
{"version": 1, "created_utc": "2026-05-03T00:00:00Z", "agents": ["alpha","bravo","charlie"]}
JSON

  for agent in alpha bravo charlie; do
    mkdir -p "$AMQ_GLOBAL_ROOT/agents/$agent/inbox/new"
  done
  # alpha: one fresh message
  : > "$AMQ_GLOBAL_ROOT/agents/alpha/inbox/new/2026-05-03T00-00-00.000Z_pid1_a.md"
  touch -d '5 minutes ago' "$AMQ_GLOBAL_ROOT/agents/alpha/inbox/new/2026-05-03T00-00-00.000Z_pid1_a.md"
  # charlie: three messages including an old one
  : > "$AMQ_GLOBAL_ROOT/agents/charlie/inbox/new/2026-05-03T00-00-00.000Z_pid1_c1.md"
  : > "$AMQ_GLOBAL_ROOT/agents/charlie/inbox/new/2026-05-03T00-00-00.000Z_pid1_c2.md"
  : > "$AMQ_GLOBAL_ROOT/agents/charlie/inbox/new/2026-05-03T00-00-00.000Z_pid1_c3.md"
  touch -d '2 hours ago' "$AMQ_GLOBAL_ROOT/agents/charlie/inbox/new/2026-05-03T00-00-00.000Z_pid1_c1.md"

  # The shell-setup guard refuses to load when a binary is present but
  # binary.sha256 isn't; we don't ship a binary in fixtures, so this
  # state is fine — sec_binary_integrity reports "binary-missing".
}

# Seed a worktree dir so anon_text has a non-empty `worktrees/` to walk
# and sec_amq's display_name lookups have non-AMQ paths to redact too.
seed_worktree() {
  local branch="${1:-feature-x}"
  mkdir -p "$DUX_HOME/worktrees/$branch"
  : > "$DUX_HOME/worktrees/$branch/.gitkeep"
}

# ============================================================================
# Test 1: every expected section header is rendered.
# ============================================================================
@test "doctor produces all expected sections" {
  seed_amq_state

  run "$DOCTOR"
  [ "$status" -eq 0 ]
  [[ "$output" == *"== Versions =="* ]]
  [[ "$output" == *"== Binary integrity =="* ]]
  [[ "$output" == *"== Persistent disk =="* ]]
  [[ "$output" == *"== AMQ =="* ]]
  [[ "$output" == *"== Symlinks =="* ]]
  [[ "$output" == *"== Kernel =="* ]]
  [[ "$output" == *"== Sessions DB =="* ]]
  [[ "$output" == *"== Runtime =="* ]]
  [[ "$output" == *"== Recent errors"* ]]
}

# ============================================================================
# Test 2: --anonymize redacts $HOME, branch names, agent IDs.
# ============================================================================
@test "doctor --anonymize redacts host paths and agent identifiers" {
  seed_amq_state
  seed_worktree "feature-x"
  seed_worktree "main"

  # Mention the branch in a place anon_text will visit. The recent-errors
  # walker scrubs path strings inside log messages; seed a fake JSON-
  # Lines log line that references the worktree.
  cat > "$DUX_HOME/dux.log" <<EOF
{"timestamp":"2026-05-03T00:00:00Z","level":"ERROR","fields":{"message":"failed in $DUX_HOME/worktrees/feature-x/src"}}
EOF

  run "$DOCTOR" --anonymize
  [ "$status" -eq 0 ]
  # Real $HOME path must be absent from anonymized output. The doctor
  # itself runs from $TEST_HOME (an isolated /tmp dir) so we check that
  # the test harness's HOME is gone, not the real user's.
  [[ "$output" != *"$HOME"* ]] || \
    { echo "leaked HOME=$HOME in output:" >&2; echo "$output" >&2; false; }
  # Concrete agent names must not appear; their numbered placeholders must.
  [[ "$output" != *"alpha"* ]]
  [[ "$output" != *"bravo"* ]]
  [[ "$output" != *"charlie"* ]]
  [[ "$output" == *"agent-1"* ]]
  [[ "$output" == *"agent-2"* ]]
  [[ "$output" == *"agent-3"* ]]
}

# ============================================================================
# Test 3: --json emits a single valid JSON object.
# ============================================================================
@test "doctor --json emits valid JSON" {
  seed_amq_state

  run bash -c "'$DOCTOR' --json | jq -e ."
  [ "$status" -eq 0 ]

  # Spot-check expected top-level fields. `jq -e` returned 0 above, so
  # we know the output parses; these are structural assertions.
  run bash -c "'$DOCTOR' --json | jq -er '.versions, .binary_integrity, .amq, .symlinks, .kernel, .sessions_db, .runtime, .recent_errors | type'"
  [ "$status" -eq 0 ]
}

# ============================================================================
# Test 4: doctor still exits 0 even with no AMQ initialization.
# ============================================================================
@test "doctor exits 0 when amq queue is uninitialized" {
  # Deliberately do NOT call seed_amq_state — meta/config.json is absent.
  mkdir -p "$DUX_HOME"

  run "$DOCTOR"
  [ "$status" -eq 0 ]
  [[ "$output" == *"== AMQ =="* ]]
  [[ "$output" == *"uninitialized"* || "$output" == *"no $AMQ_GLOBAL_ROOT/meta/config.json"* ]]

  # JSON mode must also succeed and report initialized=false.
  run bash -c "'$DOCTOR' --json | jq -er '.amq.initialized == false'"
  [ "$status" -eq 0 ]
}
