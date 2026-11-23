#!/usr/bin/env bash
# Run the browser checks against the preview.
#
#   ./run.sh                          every check
#   ./run.sh one-driver macro-picker  just these
#   ./run.sh --list                   the checks there are, and what each protects
#
# The checks are driven against the screenshot scenes' workspace, so the preview
# has to be the screenshot one (the stand-in gh, the opencode alias): one that is
# not is recreated with --restart, which keeps the volumes, and the workspace is
# seeded when it is not already the seeded one. Prints one PASS or FAIL line per
# check and exits non-zero when any failed; a failure's screenshots land in
# ./failures, which git ignores.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PREVIEW="$(cd "$HERE/.." && pwd)"
PORT="${DUX_PORT:-8790}"
export DUX_PORT="$PORT"

fail() {
  echo "error: $1" >&2
  exit 1
}

# shellcheck source=../screens/preview.sh
. "$PREVIEW/screens/preview.sh"

if [ "${1:-}" = "--list" ]; then
  (cd "$HERE" && node run.js --list)
  exit 0
fi

prepare_browser

answering() { curl -fsS --max-time 3 "http://127.0.0.1:$PORT/api/v1/workspace" > /dev/null 2>&1; }
screens_mode() { compose exec -T dux test -f /data/screens-mode > /dev/null 2>&1; }

if ! answering || ! screens_mode; then
  echo ">> bringing the preview up with the screenshot fixtures (--restart keeps its volumes)"
  (cd "$PREVIEW" && DUX_SCREENS=1 DUX_NO_TAILSCALE=0 ./up.sh --restart)
  answering || fail "the preview never answered on port $PORT"
fi

cd "$HERE"
exec node run.js "$@"
