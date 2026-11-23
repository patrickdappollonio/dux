#!/usr/bin/env bash
# Run the browser checks against the preview, built from this checkout.
#
#   ./run.sh                          every check
#   ./run.sh one-driver macro-picker  just these
#   ./run.sh --reseed [check...]      put the workspace back to the seed first
#   ./run.sh --list                   the checks there are, and what each protects
#
# The web UI is compiled into the dux binary, so a check only says something
# about the working tree when the preview runs a binary built from it. Every run
# therefore builds the release binary (a no-op when nothing changed), and
# restarts the preview with `up.sh --restart` when the container runs any other
# binary, is not the screenshot one (the stand-in gh, the opencode alias the
# seed needs), or does not answer. It never does a plain bring-up and never
# pulls: the preview image has to exist already. The workspace is seeded when it
# is not the seeded one. Prints one PASS or FAIL line per check and exits
# non-zero when any failed; a failure's screenshots land in ./failures, which
# git ignores.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PREVIEW="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$PREVIEW/../.." && pwd)"
PORT="${DUX_PORT:-8790}"
export DUX_PORT="$PORT"
BINARY="$REPO/target/release/dux"

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

# A missing image would make compose fetch one by that name from a registry, so
# it is refused here instead.
if ! docker_cmd image inspect dux-preview > /dev/null 2>&1; then
  fail "there is no dux-preview image yet (\`docker image ls dux-preview\` lists none). It is built once by a plain \`tools/preview-env/up.sh\`, which downloads a base image and packages; ask the project owner before running that."
fi

echo ">> building dux (release) from $REPO"
if ! (cd "$REPO" && cargo build --release --bin dux); then
  fail "the dux build failed (above), so no check was run: a check against the binary already in the preview would say nothing about this tree"
fi
want="$(sha256sum "$BINARY" | cut -d' ' -f1)"

answering() { curl -fsS --max-time 3 "http://127.0.0.1:$PORT/api/v1/workspace" > /dev/null 2>&1; }
screens_mode() { compose exec -T dux test -f /data/screens-mode > /dev/null 2>&1; }
running_binary() { compose exec -T dux sha256sum /usr/local/bin/dux 2> /dev/null | cut -d' ' -f1 || true; }

reason=""
if ! answering; then
  reason="nothing answers on port $PORT"
elif ! screens_mode; then
  reason="it is an ordinary preview, not the screenshot one"
elif [ "$(running_binary)" != "$want" ]; then
  reason="it runs another binary than the one just built"
fi
if [ -n "$reason" ]; then
  echo ">> restarting the preview ($reason) with the screenshot fixtures; --restart keeps its volumes"
  (cd "$PREVIEW" && DUX_SRC="$REPO" DUX_SCREENS=1 DUX_NO_TAILSCALE=0 ./up.sh --restart)
  answering || fail "the preview never answered on port $PORT"
  [ "$(running_binary)" = "$want" ] || fail "the preview still runs another binary than $BINARY after restarting it"
fi

tree="$(git -C "$REPO" rev-parse --short HEAD)"
[ -z "$(git -C "$REPO" status --porcelain)" ] || tree="$tree with uncommitted changes"
echo ">> checking $(compose exec -T dux dux --version | tr -d '\r'), binary sha256 ${want:0:12}, built from $tree"

cd "$HERE"
exec node run.js "$@"
