#!/usr/bin/env bash
#
# Smoke-test a packaged dux release archive by RUNNING it.
#
# Two layers of checking, because they fail in different circumstances.
#
# STATIC (always runs, on every archive including ones this runner cannot
# execute): the binary must contain embedded content-hashed asset NAMES. rust-embed
# stores each embedded path as a plain string literal, so `grep` on the binary sees
# them; a notice-page-only build embeds just index.html and has none. This is the
# only check that covers the cross-compiled x86_64-apple-darwin archive, which
# cannot be started on the arm64 macOS runner that builds it.
#
# RUNTIME (needs an executable archive): starts `dux server` on a spare loopback
# port against a throwaway HOME and verifies what a placeholder cannot satisfy:
#
#   1. GET / returns a page that references a content-hashed bundle
#      (assets/<name>-<hash>.<js|css>), which only a real Vite build produces.
#   2. EVERY asset in the reference graph downloads: the ones the page names, and
#      the lazily imported chunks named only INSIDE those bundles. The terminal
#      emulator and the editor's viewers are code-split, so their chunk names
#      appear nowhere in the HTML; a dist missing them serves a page that looks
#      perfect and goes blank the moment a terminal is opened. Checking only the
#      page, or only the first asset it names, cannot see that.
#   3. The graph is big enough to be a real app rather than a set of stubs.
#   4. A WebSocket upgrade on /ws/events completes with 101 Switching Protocols
#      AND the Sec-WebSocket-Accept header is the correct digest of the key we
#      sent, so this is a real WebSocket endpoint and not merely something that
#      answered.
#
# Exits non-zero on any failure so the caller can refuse to publish the release.
#
# Deliberately tests the ARCHIVE, not a fresh build: the thing users download is
# the thing that gets checked. Note the ordering honestly though. In the release
# workflow this runs after the tag already exists, so it cannot un-publish a tag.
# What it CAN do, and now does, is stop the release from becoming reachable at
# all: no archive is uploaded until every platform's smoke test has passed (see
# the publish-archives job). The other gates are that a failed frontend build
# fails `cargo build` (crates/dux-web/build.rs), that the release and PR workflows
# refuse to run with DUX_DISABLE_UI_BUILD set, and that
# crates/dux-web/tests/static_serving.rs asserts the same properties, with the
# same graph walk, on every pull request.
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

# ---------------------------------------------------------------------------
# STATIC CHECK. Runs on every archive, executable here or not.
# ---------------------------------------------------------------------------
#
# rust-embed generates a match over the embedded paths, so each one survives in
# the binary as a plain string. A real build contributes dozens of
# `assets/<name>-<hash>.<js|css>` names; the notice page contributes none, because
# a notice-only dist holds one un-hashed index.html.
#
# The floor is MEASURED, not a token "more than one": this exact grep against a
# real build of this app finds 92 distinct names (90 JavaScript chunks plus 2
# stylesheets; the 6 remaining files in assets/ are fonts, which this pattern does
# not look for). The floor was 2, which is a number a badly broken dist can reach:
# for the cross-compiled x86_64-apple-darwin archive this static check is the
# ENTIRE verification, since the script exits before every runtime check below, so
# a threshold of 2 was the only thing standing between a gutted dist and a shipped
# archive. 40 leaves more than a factor of two of headroom for the app shedding
# code-split chunks while staying far out of stub reach.
#
# The asset CONTENT cannot be checked this way (build.rs gzips it before
# embedding, so index.html's text is compressed and not greppable). The names are
# enough for what this is: proof that a real frontend build went in.
#
# -a treats the binary as text; without it grep reports "binary file matches" and
# the count is lost.
#
# The trailing `|| true` is load-bearing. grep exits 1 when it matches nothing,
# and under `set -o pipefail` that failed the whole substitution, so `set -e`
# killed the script silently at precisely the moment it had something to say. The
# count is still "0" on that path, which is what the check wants to read.
embedded_assets="$(grep -a -oE 'assets/[A-Za-z0-9_.-]+-[A-Za-z0-9_-]{8,}[.](js|css)' "$BIN" \
  | sort -u | wc -l | tr -d ' ' || true)"
MIN_EMBEDDED_ASSETS=40
if [ "$embedded_assets" -lt "$MIN_EMBEDDED_ASSETS" ]; then
  fail "the binary in $ARCHIVE embeds $embedded_assets content-hashed web assets, \
under the floor of ${MIN_EMBEDDED_ASSETS}. A real frontend build of this app embeds \
92; a 'web UI not built' notice page embeds none. This archive was built without \
the web UI, or with a dist that is missing most of it."
fi
echo "OK (static): the binary embeds $embedded_assets content-hashed web assets."

# The archive has to be executable on this runner for the rest. Every combination
# in the release matrix is native except x86_64-apple-darwin, which is
# cross-compiled on an arm64 macOS runner and would need Rosetta. Skip loudly
# rather than pretend: a green check that silently ran nothing is the class of bug
# this whole script exists to close. The static check above still covered it.
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
  echo "SKIPPED (runtime only): $TARGET cannot be executed on this runner"
  echo "         ($host_os/$host_arch), so the server cannot be started here. The"
  echo "         static check above DID run against this archive's binary. Run this"
  echo "         script on a $want_os host of a matching architecture for the rest."
  exit 0
fi

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

# 2. The whole reference graph. Start from the content-hashed bundles the page
# names, then follow the lazy-import targets found INSIDE each JavaScript bundle.
#
# The two reference forms are different and both are needed. In the HTML a bundle
# appears as `assets/<name>-<hash>.<ext>`. Inside a bundle, an `import()` target is
# written relative to that bundle's own directory: `./TerminalPane-<hash>.js`.
# Measured on a real build, the entry chunk contains ZERO `assets/` strings and 90
# distinct relative ones, which is exactly why a page-level check is blind to the
# terminal emulator and the editor.
#
# The DELIMITER matters as much as the path, and getting it wrong is what made
# this walk nearly blind. It used to require a double quote, and the bundler emits
# 85 of those 90 names as BACKTICK template literals, so the walk reached 10 of the
# 98 files in assets/: deleting the other 88, every Monaco language chunk and the
# editor's web worker included, left this check reporting nothing missing. All
# three JavaScript string delimiters therefore count, and the leading `./` is
# optional, because the worker is built from
# `new Worker(""+new URL(`editor.worker-<hash>.js`,import.meta.url))` with no
# prefix. ERE has no backreference, so the closing delimiter is the same class
# rather than the same character; a genuinely mismatched pair does not occur in
# emitted output, and anything spurious that slipped through would simply be
# fetched and found.
#
# Hash shape: at least 8 characters of [A-Za-z0-9_-] after a dash. The hash
# alphabet is base64url, so a hash can CONTAIN a dash (a real build emits
# TerminalPane-BrP-ENHg.css); matching only the final dash-separated segment
# rejects those, so the pattern allows dashes inside the hash. A hand-written
# assets/dux-logo.png still does not match, its suffix being far too short.
HASHED_IN_HTML='assets/[A-Za-z0-9_.-]+-[A-Za-z0-9_-]{8,}\.(js|css)'
QUOTE_CLASS='["'"'"'`]'
HASHED_IN_JS="${QUOTE_CLASS}(\./)?[A-Za-z0-9_.-]+-[A-Za-z0-9_-]{8,}\.(js|css)${QUOTE_CLASS}"

grep -oE "$HASHED_IN_HTML" "$WORK/index.html" | sort -u >"$WORK/queue" || true
if [ ! -s "$WORK/queue" ]; then
  echo "--- served page ---" >&2
  cat "$WORK/index.html" >&2
  fail "the served page references no content-hashed bundle, so it is not a real frontend build"
fi
echo "Page references $(wc -l <"$WORK/queue" | tr -d ' ') hashed bundle(s)."

: >"$WORK/seen"
total_bytes=0
followed_chunks=0

while [ -s "$WORK/queue" ]; do
  asset="$(head -n 1 "$WORK/queue")"
  sed -i.bak '1d' "$WORK/queue" 2>/dev/null || sed -i '1d' "$WORK/queue"
  rm -f "$WORK/queue.bak"

  grep -qxF "$asset" "$WORK/seen" 2>/dev/null && continue
  printf '%s\n' "$asset" >>"$WORK/seen"

  curl -fsS --max-time 30 "http://127.0.0.1:$PORT/$asset" -o "$WORK/asset.bin" \
    || fail "the build references /$asset but the server does not serve it. A chunk that
       404s is a blank screen the moment the feature behind it is opened."
  asset_size="$(wc -c <"$WORK/asset.bin" | tr -d ' ')"
  [ "$asset_size" -gt 64 ] \
    || fail "/$asset is only $asset_size bytes, which is not a real bundle"
  total_bytes=$((total_bytes + asset_size))

  case "$asset" in
    *.js) ;;
    *) continue ;;
  esac

  # Relative chunk names become full asset paths for the next round.
  while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    chunk="assets/$(printf '%s' "$ref" | tr -d "\"'\`" | sed 's|^\./||')"
    grep -qxF "$chunk" "$WORK/seen" 2>/dev/null && continue
    printf '%s\n' "$chunk" >>"$WORK/queue"
    followed_chunks=$((followed_chunks + 1))
  done <<EOF
$(grep -oE "$HASHED_IN_JS" "$WORK/asset.bin" | sort -u || true)
EOF
done

checked="$(wc -l <"$WORK/seen" | tr -d ' ')"
echo "Reference graph OK: $checked assets, $total_bytes bytes, $followed_chunks lazy chunk(s) followed."

# "at least one" was the canary, and it caught nothing: with the double-quote-only
# pattern above, deleting 88 of the 98 files in assets/ still left 6 chunks
# followed and this check green. Measured on a real build, the walk follows 88 and
# reaches 92 assets; 40 keeps a factor of two of headroom.
MIN_FOLLOWED_CHUNKS=40
if [ "$followed_chunks" -lt "$MIN_FOLLOWED_CHUNKS" ]; then
  fail "only $followed_chunks lazily loaded chunk(s) were found inside the bundles,
       under the floor of ${MIN_FOLLOWED_CHUNKS}. This app code-splits the terminal
       emulator, the editor and every Monaco language, so finding this few means
       either this is not a real build, chunks are missing from the dist, or the
       chunk pattern has stopped matching what the bundler emits."
fi

# An aggregate floor rather than a bigger per-file one, and the difference is
# measured: rolldown-runtime-<hash>.js is a legitimate 694-byte chunk referenced
# straight from index.html, so a per-file floor of even 1KB fails a good build. The
# real graph is about 5.5MB, so 512KB has an order of magnitude of headroom while
# staying far out of reach of a set of stubs.
MIN_TOTAL_BYTES=524288
if [ "$total_bytes" -lt "$MIN_TOTAL_BYTES" ]; then
  fail "the whole bundle graph is only $total_bytes bytes across $checked assets,
       under the ${MIN_TOTAL_BYTES}-byte floor. That is stub territory, not a real build."
fi

# 3. The WebSocket handshake. Raw sockets via python3 (present on every GitHub
# runner) rather than a new dependency; we only need the handshake, not a session.
#
# The status line alone is not enough. A 101 proves something answered; it does not
# prove a WebSocket answered. RFC 6455 makes the server echo a digest derived from
# the key the client sent, so checking Sec-WebSocket-Accept against the key WE
# generated is what distinguishes the real endpoint from anything else that
# happens to switch protocols. A fresh random key each run means a hardcoded or
# replayed value cannot satisfy it either.
python3 - "$PORT" <<'PY' || fail "the WebSocket upgrade on /ws/events did not complete"
import base64, hashlib, os, socket, sys

port = int(sys.argv[1])
key = base64.b64encode(os.urandom(16)).decode()
# The GUID is fixed by RFC 6455 section 1.3.
GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
expected = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()

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

lines = head.split("\r\n")
if "101" not in lines[0]:
    sys.exit("expected 101 Switching Protocols on /ws/events")

headers = {}
for line in lines[1:]:
    name, _, value = line.partition(":")
    headers[name.strip().lower()] = value.strip()

upgrade = headers.get("upgrade", "")
if upgrade.lower() != "websocket":
    sys.exit(f"Upgrade header is {upgrade!r}, expected 'websocket'")

accept = headers.get("sec-websocket-accept")
if accept is None:
    sys.exit("the 101 response carried no Sec-WebSocket-Accept header, so this is "
             "not a WebSocket endpoint")
if accept != expected:
    sys.exit(f"Sec-WebSocket-Accept was {accept!r} but the key we sent requires "
             f"{expected!r}; the response does not correspond to our handshake")

print(f"Sec-WebSocket-Accept matches the key sent ({accept}).")
PY

echo "OK: $ARCHIVE serves a real web UI and accepts a WebSocket upgrade."
