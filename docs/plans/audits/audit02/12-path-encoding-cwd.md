# Phase 12: Path encoding + realpath cwd containment + fixtures

> Maps to: **audit01 P0-5** (still open).

## Goal
Replace the brittle `sed` Claude-Code path encoder in `claude-amq` with
the actual encoder Claude Code uses on disk, and replace the prefix-glob
worktree containment check with a `realpath`-canonicalized comparison.
Add a fixture-based regression test so we never regress silently again.

## Pre-conditions
- Phase 00 baseline green.
- Coordinate with **Phase 10** — both phases need the encoder. Land
  Phase 12 first (single source of truth), Phase 10 imports.

## Files to touch
- `dux-amq/wrappers/claude-amq` — replace encoder + check.
- `dux-amq/wrappers/codex-amq` — symmetric containment fix.
- `dux-amq/wrappers/gemini-amq` — symmetric containment fix.
- `dux-amq/scripts/encode-claude-project-dir` — NEW, single source of
  truth for the encoder.
- `dux-amq/tests/encoder-fixtures.bats` — NEW.
- `dux-amq/tests/fixtures/claude-paths.txt` — NEW (input → expected).

## Steps

### 12.1 — Reverse-engineer the actual encoder
On a Linux VM with Claude Code installed, run a minimal session in a
known directory and observe what `~/.claude/projects/` contains:
```bash
mkdir -p /tmp/probe-1/sub_dir-with-hyphens
cd /tmp/probe-1/sub_dir-with-hyphens
claude --print 'one'   # any short oneshot
ls -la ~/.claude/projects/
```
Repeat for paths with: spaces, unicode, leading dots, trailing
slashes, very long names. Record `path → encoded-dir` pairs in
`dux-amq/tests/fixtures/claude-paths.txt`:
```
# input-path	expected-dir
/tmp/probe-1/sub_dir-with-hyphens	-tmp-probe-1-sub-dir-with-hyphens
/home/user/My Project	-home-user-My-Project          # OR escaped form
/home/user/.config/foo	-home-user--config-foo
```
Do this on a real install — do not guess.

### 12.2 — Implement the encoder
`dux-amq/scripts/encode-claude-project-dir`:
```bash
#!/usr/bin/env bash
# Encode an absolute path the way Claude Code names its projects/<dir>.
# Single source of truth for claude-amq seeding + audit02 Phase 10 purge.
#
# Encoding (verified against real installs at AMQ_PINNED_VERSION):
#   1. Strip trailing slash.
#   2. Replace each `/` with `-`.
#   3. Replace each `_` with `-`.
#   4. (any other rules discovered during 12.1)
set -euo pipefail
in="${1:?usage: encode-claude-project-dir <abs-path>}"
[[ "$in" == /* ]] || { echo "encode: absolute path required" >&2; exit 2; }
in="${in%/}"               # strip trailing slash
out="${in//\//-}"          # / → -
out="${out//_/-}"          # _ → -
# (extend rules as 12.1 fixtures dictate)
printf '%s' "$out"
```

### 12.3 — Use the encoder in claude-amq
Replace `claude-amq:38-39`:
```bash
# Before
enc_self=$(echo "$PWD"           | sed 's|/|-|g; s|_|-|g')
enc_main=$(echo "$main_worktree" | sed 's|/|-|g; s|_|-|g')

# After
enc_self=$(encode-claude-project-dir "$PWD")
enc_main=$(encode-claude-project-dir "$main_worktree")
```
(The encoder script must be on `$PATH`; install.sh installs it to
`$LOCAL_BIN`.)

### 12.4 — Replace prefix-glob containment with realpath
`claude-amq:67`, `codex-amq:12`, `gemini-amq:14` — currently:
```bash
if [[ -z "$ME" && "$PWD" == "${DUX_HOME:-/data/state/dux}/worktrees/"* ]]; then
  ME=$(basename "$PWD")
fi
```
The glob matches `/data/state/dux/worktrees-evil/x` (prefix, not path
containment). Replace with realpath canonical containment:
```bash
is_dux_worktree() {
  local pwd_real worktrees_real
  pwd_real=$(realpath -- "$PWD" 2>/dev/null) || return 1
  worktrees_real=$(realpath -- "${DUX_HOME:-/data/state/dux}/worktrees" 2>/dev/null) || return 1
  case "$pwd_real/" in
    "$worktrees_real"/*) return 0 ;;
    *) return 1 ;;
  esac
}

if [[ -z "$ME" ]] && is_dux_worktree; then
  ME=$(basename "$PWD")
fi
```
The trailing slash in `"$pwd_real/"` plus the `/*` pattern enforces a
real path-segment boundary, not a string prefix.

### 12.5 — Tests
`dux-amq/tests/encoder-fixtures.bats`:
```bash
load lib/setup
@test "encoder matches recorded fixtures" {
  while IFS=$'\t' read -r input expected; do
    [[ "$input" =~ ^# ]] || [[ -z "$input" ]] && continue
    actual=$(dux-amq/scripts/encode-claude-project-dir "$input")
    [[ "$actual" == "$expected" ]] || {
      echo "encode($input) = $actual, expected $expected" >&2
      false
    }
  done < dux-amq/tests/fixtures/claude-paths.txt
}
@test "is_dux_worktree rejects sibling /worktrees-evil" {
  mkdir -p /tmp/dux-state/worktrees-evil/x
  pushd /tmp/dux-state/worktrees-evil/x
  DUX_HOME=/tmp/dux-state run is_dux_worktree
  [ "$status" -ne 0 ]
  popd
}
@test "is_dux_worktree accepts /worktrees/x" {
  mkdir -p /tmp/dux-state/worktrees/x
  pushd /tmp/dux-state/worktrees/x
  DUX_HOME=/tmp/dux-state run is_dux_worktree
  [ "$status" -eq 0 ]
  popd
}
```

## Validation
- `make overlay-test` green.
- Manual: in `/data/state/dux/worktrees/some-feat`, `claude-amq` should
  set `ME=some-feat` and seed correctly. In `/data/state/dux/worktrees-evil/x`,
  `claude-amq` should fall through to git-branch / `claude-$$`.
- Compare `~/.claude/projects/` listing on a fresh install against the
  encoder's output for the same paths — must match.

## Acceptance criteria
- [ ] `encode-claude-project-dir` script exists, installed to `$LOCAL_BIN`.
- [ ] Wrappers use the encoder (no inline `sed` paths).
- [ ] All three wrappers replace prefix-glob with `realpath`
      containment via `is_dux_worktree()`.
- [ ] Fixture file `claude-paths.txt` has at least 6 entries covering
      hyphens, underscores, dotted paths, trailing slash.
- [ ] `encoder-fixtures.bats` passes locally and in CI.
- [ ] PR: `fix(wrappers): correct path encoder + realpath cwd check (audit01 P0-5)`.

## Known pitfalls
- Claude Code's encoder MAY vary by version. Pin against the AMQ
  pinned Claude version; document in encoder script header. If
  Anthropic changes encoding, fixtures will fail and we'll re-derive.
- macOS realpath has different flags vs GNU. Use `realpath --` (GNU)
  on Linux; on macOS, `grealpath` from `coreutils` brew. Add a
  preflight check in `install.sh` (Phase 02 already preflights tools;
  add `realpath`).
- `realpath` resolves symlinks; if a user's worktree path is itself a
  symlink, the canonicalised form may not match `DUX_HOME/worktrees/x`.
  Document: "use absolute, non-symlinked paths for `DUX_HOME`."
- The encoder may need additional rules (capitalization, dot-prefixed
  components). Don't speculate — drive from observed fixtures.

## References
- audit01 P0-5.
- audit02 Phase 10 (purge encoder dependency).
