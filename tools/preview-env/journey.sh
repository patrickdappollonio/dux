#!/usr/bin/env bash
# Run a named journey (a JSON action sequence under journeys/) against the
# running preview container, then screenshot the result.
#
#   ./journey.sh <name> [out.png] [--mobile]     # journeys/<name>.json
#   ./journey.sh working-agent working.png
#
# The action DSL is drive.js's: click / clickSel / type / typeInto / key /
# wait / hover. Entries whose only key is "comment" are ignored, so journeys
# can document themselves. Add new journeys as plain JSON files; nothing else
# to register.
set -euo pipefail
cd "$(dirname "$0")"

NAME="${1:?usage: ./journey.sh <journey-name> [out.png] [--mobile]}"
OUT="${2:-$NAME.png}"
shift || true
shift || true

FILE="journeys/$NAME.json"
[ -f "$FILE" ] || {
  echo "error: $FILE not found; available journeys:" >&2
  ls journeys/*.json 2> /dev/null | sed 's|journeys/||; s|\.json$||' >&2
  exit 1
}

PORT="${DUX_PORT:-8790}"

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

[ -d node_modules/puppeteer-core ] || npm install --silent

# Strip self-documentation entries before handing the actions to drive.js.
ACTIONS="$(node -e '
  const a = require("./'"$FILE"'").filter(x => !(Object.keys(x).length === 1 && "comment" in x));
  process.stdout.write(JSON.stringify(a));
')"

CHROME="$CHROME_BIN" node drive.js "http://127.0.0.1:$PORT/" "$OUT" "$ACTIONS" "$@"
echo ">> $OUT"
