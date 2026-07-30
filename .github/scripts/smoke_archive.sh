#!/usr/bin/env bash
#
# Smoke-test a packaged dux release archive by RUNNING it.
#
# Unpacks the archive, starts `dux server` on a spare loopback port against a
# throwaway HOME, and verifies three things a placeholder build cannot satisfy:
#
#   1. GET / returns a page that references a content-hashed bundle
#      (assets/<name>-<hash>.<js|css>), which only a real Vite build produces.
#   2. That bundle actually downloads and is not a stub.
#   3. A WebSocket upgrade on /ws/events completes with 101 Switching Protocols,
#      so the server is live and the static fallback has not swallowed the socket.
#
# Exits non-zero on any failure so the caller can refuse to upload the archive.
#
# Deliberately tests the ARCHIVE, not a fresh build: the thing users download is
# the thing that gets checked. Note the ordering honestly though. In the release
# workflow this runs after the tag already exists, so it is the LAST line of
# defence, not the primary gate. The primary gates are that a failed frontend
# build fails `cargo build` (crates/dux-web/build.rs), that the release workflow
# refuses to run with DUX_DISABLE_UI_BUILD set, and that
# crates/dux-web/tests/static_serving.rs asserts the same three properties on
# every pull request.
#
# Usage: .github/scripts/smoke_archive.sh <archive.tar.gz> <target-triple> [port]

set -euo pipefail

ARCHIVE="${1:?usage: smoke_archive.sh <archive.tar.gz> <target-triple> [port]}"
TARGET="${2:?usage: smoke_archive.sh <archive.tar.gz> <target-triple> [port]}"
PORT="${3:-47653}"

if [ ! -f "$ARCHIVE" ]; then
  echo "FAIL: archive $ARCHIVE does not exist" >&2
  exit 1
fi

# The archive has to be executable on this runner. Every combination in the
# release matrix is native except x86_64-apple-darwin, which is cross-compiled on
# an arm64 macOS runner and would need Rosetta. Skip loudly rather than pretend:
# a green check that silently ran nothing is the class of bug this whole script
# exists to close.
host_arch="$(uname -m)"
host_os="$(uname -s)"
case "$TARGET" in
  x86_64-*) want_arch="x86_64 amd64" ;;
  aarch64-*) want_arch="aarch64 arm64" ;;
  *) want_arch="" ;;
esac
case "$TARGET" in
  *-linux-*) want_os="Linux" ;;
  *-apple-darwin) want_os="Darwin" ;;
  *) want_os="" ;;
esac

if [ -z "$want_arch" ] || [ -z "$want_os" ]; then
  echo "SKIPPED: do not know how to check whether $TARGET runs on this host." >&2
  echo "         Add it to smoke_archive.sh rather than leaving it unchecked." >&2
  exit 0
fi
if [ "$host_os" != "$want_os" ] || ! printf '%s\n' $want_arch | grep -qx "$host_arch"; then
  echo "SKIPPED: $TARGET cannot be executed on this runner ($host_os/$host_arch),"
  echo "         so the archive cannot be started here. This archive is covered by"
  echo "         the pull-request gates only. Run this script on a $want_os host of"
  echo "         a matching architecture to smoke-test it."
  exit 0
fi

WORK="$(mktemp -d)"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    # dux traps SIGTERM and tears its PTYs down; give it a moment before SIGKILL.
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$SERVER_PID" 2>/dev/null || break
      sleep 0.5
    done
    kill -9 "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  if [ -f "$WORK/server.log" ]; then
    echo "--- dux server output ---" >&2
    cat "$WORK/server.log" >&2
    echo "--- end dux server output ---" >&2
  fi
  exit 1
}

tar xzf "$ARCHIVE" -C "$WORK" || fail "could not unpack $ARCHIVE"
BIN="$WORK/dux"
[ -x "$BIN" ] || fail "$ARCHIVE contains no executable 'dux' at its root"

# A throwaway config directory: dux keeps config, its SQLite store, its log and
# its single-instance lock under HOME ($XDG_CONFIG_HOME/dux or ~/.config/dux on
# Linux, ~/.dux on macOS). Pointing HOME at scratch keeps the smoke run from
# touching or locking a real one.
export HOME="$WORK/home"
unset XDG_CONFIG_HOME
mkdir -p "$HOME"

echo "Starting $ARCHIVE ($TARGET) on 127.0.0.1:$PORT"
"$BIN" server --bind "127.0.0.1:$PORT" --no-tailscale >"$WORK/server.log" 2>&1 &
SERVER_PID=$!

# Wait for the server to answer, and notice early if it died instead of binding.
ready=""
for _ in $(seq 1 60); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    fail "dux server exited before it started listening"
  fi
  if curl -fsS --max-time 2 "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
    ready="yes"
    break
  fi
  sleep 0.5
done
[ -n "$ready" ] || fail "dux server never answered /healthz on port $PORT within 30s"

# 1. The page.
curl -fsS --max-time 10 "http://127.0.0.1:$PORT/" -o "$WORK/index.html" \
  || fail "GET / failed"
if grep -qi 'dux-ui-not-built-notice\|DUX_DISABLE_UI_BUILD' "$WORK/index.html"; then
  fail "the served page is the 'web UI not built' notice, so this archive has no web UI"
fi

# 2. A content-hashed bundle it references. Vite emits <name>-<hash>.<ext> with an
# 8-character hash; a hand-written name (assets/dux-logo.png) does not match.
asset="$(grep -oE 'assets/[A-Za-z0-9_]+(-[A-Za-z0-9_]+)*-[A-Za-z0-9_]{8,}\.(js|css)' \
  "$WORK/index.html" | head -n 1 || true)"
if [ -z "$asset" ]; then
  echo "--- served page ---" >&2
  cat "$WORK/index.html" >&2
  fail "the served page references no content-hashed bundle, so it is not a real frontend build"
fi
echo "Page references hashed bundle: /$asset"

curl -fsS --max-time 10 "http://127.0.0.1:$PORT/$asset" -o "$WORK/asset.bin" \
  || fail "the page references /$asset but the server does not serve it"
asset_size="$(wc -c <"$WORK/asset.bin" | tr -d ' ')"
[ "$asset_size" -gt 64 ] || fail "/$asset is only $asset_size bytes, which is not a real bundle"
echo "Hashed bundle downloaded: $asset_size bytes"

# 3. The WebSocket handshake. Raw sockets via python3 (present on every GitHub
# runner) rather than a new dependency; we only need the 101, not a full session.
python3 - "$PORT" <<'PY' || fail "the WebSocket upgrade on /ws/events did not complete"
import base64, os, socket, sys

port = int(sys.argv[1])
key = base64.b64encode(os.urandom(16)).decode()
request = (
    "GET /ws/events HTTP/1.1\r\n"
    f"Host: 127.0.0.1:{port}\r\n"
    "Upgrade: websocket\r\n"
    "Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {key}\r\n"
    "Sec-WebSocket-Version: 13\r\n"
    "\r\n"
).encode()

with socket.create_connection(("127.0.0.1", port), timeout=10) as sock:
    sock.sendall(request)
    response = b""
    while b"\r\n\r\n" not in response and len(response) < 8192:
        chunk = sock.recv(4096)
        if not chunk:
            break
        response += chunk

head = response.split(b"\r\n\r\n", 1)[0].decode("latin-1")
print("WebSocket handshake response:")
print(head)
if "101" not in head.split("\r\n")[0]:
    sys.exit("expected 101 Switching Protocols on /ws/events")
PY

echo "OK: $ARCHIVE serves a real web UI and accepts a WebSocket upgrade."
