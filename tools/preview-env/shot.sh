#!/usr/bin/env bash
# Host-side wrapper around shot.js: finds a Chromium, resolves the preview
# URL, then screenshots the running container.
#
#   ./shot.sh [path] [out.png] [--mobile]        # path defaults to "/"
#   ./shot.sh '/#/agent/<sid>' agent.png
#   ./shot.sh / home-mobile.png --mobile
set -euo pipefail
cd "$(dirname "$0")"

PORT="${DUX_PORT:-8790}"

# Chromium discovery, in preference order: an explicit CHROME=, a cached
# Playwright build, then common system binaries.
CHROME_BIN="${CHROME:-}"
if [ -z "$CHROME_BIN" ]; then
  CHROME_BIN="$(ls "$HOME"/.cache/ms-playwright/chromium-*/chrome-linux64/chrome 2>/dev/null | head -1 || true)"
fi
if [ -z "$CHROME_BIN" ]; then
  for c in chromium chromium-browser google-chrome google-chrome-stable; do
    if command -v "$c" > /dev/null 2>&1; then
      CHROME_BIN="$(command -v "$c")"
      break
    fi
  done
fi
if [ -z "$CHROME_BIN" ] || [ ! -x "$CHROME_BIN" ]; then
  echo "error: no Chromium found; set CHROME=<path-to-chrome>" >&2
  exit 1
fi

# First arg may be a page path (starts with /) or already a full URL.
first="${1:-/}"
shift || true
case "$first" in
  http://* | https://*) url="$first" ;;
  *) url="http://127.0.0.1:$PORT$first" ;;
esac

[ -d node_modules/puppeteer-core ] || npm install --silent

CHROME="$CHROME_BIN" node shot.js "$url" "$@"
