#!/usr/bin/env bash
# Build the dux binary on the host, then bring up the isolated preview
# container. Point DUX_SRC at whichever checkout/worktree you want to preview;
# it defaults to the repository this script lives in.
#
#   ./up.sh                 # build + start (this repo)
#   DUX_SRC=/path ./up.sh   # preview a different worktree
#   ./up.sh --restart       # rebuild binary + restart container (no image rebuild)
#
# Docker only, on purpose. Never run `dux server` directly on a development
# host to inspect the UI: it would share the developer's real ~/.config/dux
# (their live instance may be running), and a stray instance or a killed dux
# process can destroy a live session. The container's state lives in named
# volumes and cannot reach the host's config.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="${DUX_SRC:-$(cd "$HERE/../.." && pwd)}"
PORT="${DUX_PORT:-8790}"
BASE_IMAGE="${BASE_IMAGE:-archlinux:latest}"

# The mount-the-host-binary path only works when the host builds a Linux
# binary of the container's arch. On macOS a Mach-O binary cannot run in the
# Linux container; build in-container instead (see README). Fail loudly
# rather than mount garbage.
if [ "$(uname -s)" = "Darwin" ]; then
  echo "error: this host-build+mount path is Linux-only. On macOS, build dux" >&2
  echo "       in-container (see README 'in-container build')." >&2
  exit 1
fi

echo ">> building dux (release) from $SRC"
(cd "$SRC" && cargo build --release --bin dux)
DUX_BIN="$SRC/target/release/dux"
[ -x "$DUX_BIN" ] || { echo "error: $DUX_BIN not built" >&2; exit 1; }
echo ">> DUX_BIN=$DUX_BIN"

# Run docker directly when this shell already has access; fall back to
# `sg docker` for shells whose user was added to the docker group without a
# re-login. `sg` needs the env inlined so it survives the sub-shell.
if docker info > /dev/null 2>&1; then
  run_docker() { (cd "$HERE" && DUX_BIN="$DUX_BIN" DUX_PORT="$PORT" BASE_IMAGE="$BASE_IMAGE" docker compose "$@"); }
else
  run_docker() {
    sg docker -c "cd '$HERE' && DUX_BIN='$DUX_BIN' DUX_PORT='$PORT' BASE_IMAGE='$BASE_IMAGE' docker compose $*"
  }
fi

if [ "${1:-}" = "--restart" ]; then
  # force-recreate re-resolves the bind-mount source, so the container picks
  # up the freshly built binary's new inode.
  echo ">> recreating container with the freshly built binary"
  run_docker up -d --no-build --force-recreate dux
else
  echo ">> building image + starting container"
  run_docker up -d --build
fi

echo ">> dux preview at http://127.0.0.1:$PORT"
echo ">> logs:  docker compose logs -f dux   (from $HERE)"
echo ">> shot:  ./shot.sh / home.png"
