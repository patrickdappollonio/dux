# shellcheck shell=bash
# Append these to ~/.bashrc to wire dux + AMQ into your shell.
# install.sh substitutes REPLACE_AT_INSTALL with the overlay version
# (audit01 Phase 12) before appending; the >>>/<<< markers are then used
# by strip_block() to delete-and-rewrite on every re-install, so version
# bumps actually propagate.

# >>> dux-amq vREPLACE_AT_INSTALL >>>
export DUX_HOME="${DUX_HOME:-/data/state/dux}"
export AMQ_GLOBAL_ROOT="${AMQ_GLOBAL_ROOT:-/data/state/amq}"
# Audit01 P1-8: pinned amq binary path + recorded sha256. The guard below
# refuses to `eval` shell-setup output if the on-disk binary no longer
# matches the install-time hash — `eval` runs every interactive shell, so
# this is a meaningful trust narrowing even though we already pin at
# install time.
export AMQ_BIN="${AMQ_BIN:-/data/state/amq-bin/amq}"
_amq_shell_setup_guarded() {
  local rec="${AMQ_GLOBAL_ROOT:-/data/state/amq}/binary.sha256"
  # No binary yet → install hasn't run, nothing to guard. Quietly skip.
  if [[ ! -x "$AMQ_BIN" ]]; then
    return 0
  fi
  # Audit02 N-3: previously this was a silent `return 0` if either the
  # binary OR the record was missing. That's fail-open: an attacker with
  # filesystem access could disable the guard by `rm`ing one file. Now
  # we fail *closed* whenever the binary exists but the record doesn't.
  if [[ ! -f "$rec" ]]; then
    printf '\033[1;31m!\033[0m [dux-amq] amq binary present but %s missing — refusing to source shell-setup.\n' "$rec" >&2
    printf '            re-run install.sh (or rm "%s" if intentional).\n' "$AMQ_BIN" >&2
    return 1
  fi
  # Audit02 P1-E: refuse when the binary mtime is newer than the recorded
  # hash file. The full sha256 below would still catch a tampered binary,
  # but a mismatch is a confusing error message; a "binary newer than
  # recorded hash" message points the operator straight at the fix
  # (re-run install.sh, which re-records the hash). Common cases:
  #   * `apt upgrade` swapped /usr/local/bin/amq under our feet
  #   * an out-of-band `cp` over $AMQ_BIN by another tool
  #   * `setfacl`/timestamp resurrection — won't fool sha256, but mtime
  #     check fires first and produces a clearer banner.
  if [[ "$AMQ_BIN" -nt "$rec" ]]; then
    printf '\033[1;31m!\033[0m [dux-amq] amq binary newer than recorded hash (%s) — re-run install.sh\n' "$rec" >&2
    printf '            (binary: %s)\n' "$AMQ_BIN" >&2
    return 1
  fi
  local exp act
  exp=$(awk '{print $1}' "$rec")
  act=$(sha256sum "$AMQ_BIN" 2>/dev/null | awk '{print $1}')
  if [[ -z "$act" || "$exp" != "$act" ]]; then
    printf '\033[1;31m!\033[0m [dux-amq] amq binary sha mismatch (got %s, expected %s); shell-setup skipped\n' \
      "${act:-<unreadable>}" "$exp" >&2
    return 1
  fi
  # Hash matches — safe to eval.
  eval "$("$AMQ_BIN" shell-setup)"
}
_amq_shell_setup_guarded
# Optional YOLO toggle for dux panes:
# export CLAUDE_YOLO=1
# <<< dux-amq vREPLACE_AT_INSTALL <<<
