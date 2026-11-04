#!/usr/bin/env bash
# Build the dux binary on the host, then bring up the isolated preview
# container. Point DUX_SRC at whichever checkout/worktree you want to preview;
# it defaults to the repository this script lives in.
#
#   ./up.sh                 # build + start (this repo)
#   DUX_SRC=/path ./up.sh   # preview a different worktree
#   ./up.sh --restart       # rebuild binary + restart container (no image rebuild)
#   DUX_PORT=9000 ./up.sh   # move both published ports (web 9000, TUI 9208)
#   COMPOSE_PROJECT_NAME=b DUX_PORT=9000 ./up.sh   # a second stack beside the first
#
# Two loopback ports are published; see the DUX_TUI_PORT block below and the
# README for how they move together. shot.sh reads DUX_PORT independently, so
# export it (or pass it to both) when you move the web port.
#
# Docker only, on purpose. Never run `dux server` directly on a development
# host to inspect the UI: it would share the developer's real ~/.config/dux
# (their live instance may be running), and a stray instance or a killed dux
# process can destroy a live session. The container's state lives in named
# volumes and cannot reach the host's config.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="${DUX_SRC:-$(cd "$HERE/../.." && pwd)}"
DUX_PORT="${DUX_PORT:-8790}"

# Refuse a port that is not a plain decimal number, BEFORE the release build:
# the arithmetic below is the only thing that would notice, an arithmetic error
# is not fatal under `set -e` inside a `${x:-...}` default, and the stack would
# then come up on the pinned fallback port after several minutes of cargo. A
# leading zero is rejected for the same reason: the shell reads 09000 as octal
# and refuses the digit 9.
require_port() {
  case "$2" in
    '' | *[!0-9]*) ;;
    0*) ;;
    *) return 0 ;;
  esac
  echo "error: $1 must be a plain decimal port number with no leading zero," >&2
  echo "       got '$2'" >&2
  exit 1
}
require_port DUX_PORT "$DUX_PORT"
if [ -n "${DUX_TUI_PORT:-}" ]; then
  require_port DUX_TUI_PORT "$DUX_TUI_PORT"
fi

# The concurrent-TUI journey's port rides along with the web port: both are
# published, so leaving this one pinned would collide the moment a second stack
# moved DUX_PORT to get out of the first one's way. The offset keeps the pair
# together (8790 -> 8998, the historical default) and moves them as one; set
# DUX_TUI_PORT explicitly to break the pairing.
DUX_TUI_PORT="${DUX_TUI_PORT:-$((DUX_PORT + 208))}"
BASE_IMAGE="${BASE_IMAGE:-archlinux:latest}"

# The container's name. Resolved below, once compose can be asked which project
# this is; empty until then, which compose reads as its own default.
DUX_PREVIEW_CONTAINER="${DUX_PREVIEW_CONTAINER:-}"
project_name=""

# The screenshot tool's two switches, resolved here so both docker paths below
# carry them: the `sg` path builds its own environment and would otherwise drop
# whatever the caller exported. Off unless reshoot.sh set them, or unless a
# container is already running with them on (see `carry_modes_forward`).
#
# Whether the CALLER asked is recorded before defaulting, because "unset" and
# "set to the default" mean different things here: unset means "whatever this
# stack already is", and 0 means "make it an ordinary preview".
CALLER_SCREENS="${DUX_SCREENS:-}"
CALLER_NO_TAILSCALE="${DUX_NO_TAILSCALE:-}"
DUX_SCREENS="${DUX_SCREENS:-0}"
DUX_NO_TAILSCALE="${DUX_NO_TAILSCALE:-1}"

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
# re-login. `sg` takes one command STRING, so the arguments are requoted with
# printf '%q' rather than pasted in raw; pasted raw, an argument holding
# whitespace would be re-split inside the sub-shell.
if docker info > /dev/null 2>&1; then
  run_docker() {
    (
      cd "$HERE"
      export DUX_BIN DUX_PORT DUX_TUI_PORT BASE_IMAGE DUX_SCREENS DUX_NO_TAILSCALE DUX_PREVIEW_CONTAINER
      docker compose "$@"
    )
  }
  docker_raw() { docker "$@"; }
else
  run_docker() {
    local quoted
    local project=""
    quoted=$(printf '%q ' "$@")
    # Exported only when set, so an unset project keeps compose's own default
    # (this directory's name) rather than being pinned to an empty string.
    if [ -n "${COMPOSE_PROJECT_NAME:-}" ]; then
      project="export COMPOSE_PROJECT_NAME=$(printf '%q' "$COMPOSE_PROJECT_NAME") && "
    fi
    sg docker -c "cd $(printf '%q' "$HERE") && ${project}export DUX_PREVIEW_CONTAINER=$(printf '%q' "$DUX_PREVIEW_CONTAINER") DUX_BIN=$(printf '%q' "$DUX_BIN") DUX_PORT=$(printf '%q' "$DUX_PORT") DUX_TUI_PORT=$(printf '%q' "$DUX_TUI_PORT") BASE_IMAGE=$(printf '%q' "$BASE_IMAGE") DUX_SCREENS=$(printf '%q' "$DUX_SCREENS") DUX_NO_TAILSCALE=$(printf '%q' "$DUX_NO_TAILSCALE") && docker compose $quoted"
  }
  docker_raw() {
    local quoted
    quoted=$(printf '%q ' "$@")
    sg docker -c "docker $quoted"
  }
fi

# The container's name, the one thing compose does not scope to the project: a
# daemon holds one container per name, so a second stack under a project of its
# own would otherwise collide with the first on `dux-preview`. Derived from the
# project compose itself resolves (an exported COMPOSE_PROJECT_NAME, a `.env`
# file here, or this directory's name), so naming the project is all a second
# stack needs; the default project keeps `dux-preview`, as it always was. Set
# DUX_PREVIEW_CONTAINER to choose the name outright.
if [ -z "$DUX_PREVIEW_CONTAINER" ]; then
  project_name=$(run_docker config 2> /dev/null | sed -n 's/^name: //p' | head -1)
  [ -n "$project_name" ] || { echo "error: docker compose could not resolve this stack's project" >&2; exit 1; }
  if [ "$project_name" = "preview-env" ]; then
    DUX_PREVIEW_CONTAINER=dux-preview
  else
    DUX_PREVIEW_CONTAINER="dux-preview-$project_name"
  fi
fi

# What the container this project already has was brought up with, or empty.
#
# Found through compose rather than by name, because the name is only what THIS
# shell would call it: a stack started under another name (an explicit
# DUX_PREVIEW_CONTAINER) is still this project's container,
# and missing it would quietly recreate a screenshot stack as a plain one.
# `docker container inspect`, not `docker inspect`: the bare form falls back to
# any other object with that id or name, images included. The `|| true` matters
# as much: with no container at all the substitution's failure ends the script
# under `set -e`, silently, several minutes of release build after anyone could
# have noticed.
running_mode() {
  local id env
  id=$(run_docker ps -a -q dux 2> /dev/null | head -1 || true)
  [ -n "$id" ] || return 0
  env=$(docker_raw container inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$id" 2> /dev/null || true)
  printf '%s\n' "$env" | sed -n "s/^$1=//p" | head -1
}

# Bringing the stack up recreates the container from THIS shell's environment,
# so without this a plain `./up.sh --restart` of a screenshot container silently
# turns it into an ordinary preview: the stand-in gh, the transcript provider
# and the opencode alias are gone and `--no-tailscale` is back. The scenes are
# then shot against a workspace the seed never built (a provider tab that cannot
# launch the real login-walled CLI, no pull request), which is a picture of the
# wrong thing rather than a failure anybody sees. So what the stack already is
# carries forward, and only an explicit value changes it.
carry_modes_forward() {
  local inherited
  if [ -z "$CALLER_SCREENS" ]; then
    inherited="$(running_mode DUX_SCREENS)"
    if [ -n "$inherited" ] && [ "$inherited" != "$DUX_SCREENS" ]; then
      DUX_SCREENS="$inherited"
      echo ">> carrying DUX_SCREENS=$inherited forward from the running container"
      echo "   (pass DUX_SCREENS=0 to make this an ordinary preview again)"
    fi
  fi
  if [ -z "$CALLER_NO_TAILSCALE" ]; then
    inherited="$(running_mode DUX_NO_TAILSCALE)"
    if [ -n "$inherited" ] && [ "$inherited" != "$DUX_NO_TAILSCALE" ]; then
      DUX_NO_TAILSCALE="$inherited"
      echo ">> carrying DUX_NO_TAILSCALE=$inherited forward from the running container"
    fi
  fi
}

# Answering, not merely started: a container that is up is a dux still opening
# its database and sweeping its agents, and a seed or a page load aimed at it in
# that window is a race nobody can see afterwards.
wait_until_answering() {
  local seconds=0
  while [ "$seconds" -lt 120 ]; do
    if curl -fsS --max-time 3 "http://127.0.0.1:$DUX_PORT/api/v1/workspace" > /dev/null 2>&1; then
      return 0
    fi
    sleep 2
    seconds=$((seconds + 2))
  done
  echo "error: dux never answered on port $DUX_PORT; try: ${COMPOSE_PROJECT_NAME:+COMPOSE_PROJECT_NAME=$COMPOSE_PROJECT_NAME }docker compose logs dux" >&2
  exit 1
}

carry_modes_forward

if [ "${1:-}" = "--restart" ]; then
  # force-recreate re-resolves the bind-mount source, so the container picks
  # up the freshly built binary's new inode.
  echo ">> recreating container with the freshly built binary"
  run_docker up -d --no-build --force-recreate dux
else
  echo ">> building image + starting container"
  run_docker up -d --build
fi

wait_until_answering

echo ">> dux preview at http://127.0.0.1:$DUX_PORT (TUI-journey port $DUX_TUI_PORT)"
echo ">> container: $DUX_PREVIEW_CONTAINER${project_name:+ (compose project $project_name)}"
echo ">> logs:  ${COMPOSE_PROJECT_NAME:+COMPOSE_PROJECT_NAME=$COMPOSE_PROJECT_NAME }docker compose logs -f dux   (from $HERE)"
echo ">> shot:  ./shot.sh / home.png"
