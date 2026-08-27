#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="${DUX_SRC:-$(cd "$HERE/../.." && pwd)}"

usage() {
  echo "usage: ./tui-shot.sh [journey.js] [output.png] [--cols N] [--rows N] [--theme NAME]"
}

fail() {
  echo "$1" >&2
  exit "${2:-64}"
}

find_chromium() {
  if [ -n "${CHROME:-}" ]; then
    echo "$CHROME"
    return
  fi

  local executable
  executable="$(find "$HOME/.cache/ms-playwright" -path '*/chrome-linux64/chrome' -type f 2>/dev/null | head -1 || true)"
  if [ -n "$executable" ]; then
    echo "$executable"
    return
  fi

  for candidate in chromium chromium-browser google-chrome google-chrome-stable; do
    if command -v "$candidate" >/dev/null 2>&1; then
      command -v "$candidate"
      return
    fi
  done
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi
JOURNEY="${1:-$HERE/tui-journey.example.js}"
OUT="${2:-$HERE/shots/tui.png}"
if [ "$#" -ge 2 ]; then shift 2; else shift "$#"; fi

COLS=160
ROWS=45
THEME=catppuccin-mocha
while [ "$#" -gt 0 ]; do
  case "$1" in
    --cols) COLS=${2:?missing value for --cols}; shift 2 ;;
    --rows) ROWS=${2:?missing value for --rows}; shift 2 ;;
    --theme) THEME=${2:?missing value for --theme}; shift 2 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *) fail "unknown option: $1" ;;
  esac
done

case "$COLS:$ROWS" in
  *[!0-9:]*|:*) fail "invalid terminal size: ${COLS}x${ROWS}" ;;
esac
[[ "$OUT" == *.png ]] || fail "output must end in .png: $OUT"

JOURNEY=$(realpath "$JOURNEY")
[[ "$JOURNEY" == *.js ]] || fail "journey must be a JavaScript file: $JOURNEY"
[ -f "$JOURNEY" ] || fail "journey not found: $JOURNEY"

OUT=$(realpath -m "$OUT")
OUTPUT_DIR=$(dirname "$OUT")
OUTPUT_STEM=$(basename "$OUT" .png)
mkdir -p "$OUTPUT_DIR"

CHROME_BIN=$(find_chromium)
[ -x "$CHROME_BIN" ] || fail "no Chromium found; set CHROME=<path>" 1

echo ">> building dux (release) from $SRC"
(cd "$SRC" && cargo build --release --bin dux)
DUX_BIN="$SRC/target/release/dux"
REVISION=$(git -C "$SRC" rev-parse --short HEAD)

if [ ! -d "$HERE/node_modules/@xterm/xterm" ]; then
  (cd "$HERE" && npm install --silent)
fi

if docker info >/dev/null 2>&1; then
  (
    cd "$HERE"
    export DUX_BIN DUX_TUI_OUTPUT_DIR="$OUTPUT_DIR" DUX_TUI_JOURNEY="$JOURNEY"
    docker compose --profile capture build tui-shot
    docker compose --profile capture run --rm --no-deps \
      -e DUX_TUI_COLS="$COLS" \
      -e DUX_TUI_ROWS="$ROWS" \
      -e DUX_TUI_THEME="$THEME" \
      -e DUX_TUI_OUTPUT_STEM="$OUTPUT_STEM" \
      -e DUX_PREVIEW_REVISION="$REVISION" \
      -e DUX_TUI_JOURNEY_NAME="$(basename "$JOURNEY")" \
      tui-shot
  )
else
  command -v sg >/dev/null 2>&1 || fail "docker access denied and sg is unavailable" 1
  quoted=$(printf '%q ' "$DUX_BIN" "$OUTPUT_DIR" "$JOURNEY" "$COLS" "$ROWS" "$THEME" "$OUTPUT_STEM" "$REVISION" "$(basename "$JOURNEY")")
  sg docker -c "cd '$HERE' && set -- $quoted && export DUX_BIN=\"\$1\" DUX_TUI_OUTPUT_DIR=\"\$2\" DUX_TUI_JOURNEY=\"\$3\" && docker compose --profile capture build tui-shot && docker compose --profile capture run --rm --no-deps -e DUX_TUI_COLS=\"\$4\" -e DUX_TUI_ROWS=\"\$5\" -e DUX_TUI_THEME=\"\$6\" -e DUX_TUI_OUTPUT_STEM=\"\$7\" -e DUX_PREVIEW_REVISION=\"\$8\" -e DUX_TUI_JOURNEY_NAME=\"\$9\" tui-shot"
fi

CHROME="$CHROME_BIN" node "$HERE/tui-shot.js" \
  "$OUTPUT_DIR/$OUTPUT_STEM.ansi" "$OUT" "$COLS" "$ROWS" \
  "$SRC/crates/dux-web/web/src/assets/fonts/dux-mono-regular.woff2"

echo ">> PNG:      $OUT"
echo ">> text:     $OUTPUT_DIR/$OUTPUT_STEM.txt"
echo ">> metadata: $OUTPUT_DIR/$OUTPUT_STEM.json"
