#!/usr/bin/env bash
# Regenerate the docs screenshots under website/public/screens.
#
#   ./reshoot.sh                     every scene
#   ./reshoot.sh phone-hub pr-banner just these
#   ./reshoot.sh --list              the scenes there are
#
# One scene file per PNG lives in ./scenes, named after the picture it makes, so
# `grep -rn <screenshot-name> scenes` finds the journey behind any image in the
# docs. Browser scenes are driven through puppeteer against the preview
# container; terminal UI scenes are journeys for ../tui-shot.sh.
#
# The container is brought up with the screenshot fixtures installed (a stand-in
# gh and a transcript provider) and without --no-tailscale, which is what the
# Preferences shot needs; an already-running plain preview is recreated.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PREVIEW="$(cd "$HERE/.." && pwd)"
SRC="${DUX_SRC:-$(cd "$PREVIEW/../.." && pwd)}"
SCREENS="$SRC/website/public/screens"
PORT="${DUX_PORT:-8790}"

export DUX_SCREENS=1
export DUX_NO_TAILSCALE=0

fail() {
  echo "error: $1" >&2
  exit 1
}

# Docker access, resolved the way up.sh resolves it.
if docker info > /dev/null 2>&1; then
  compose() { (cd "$PREVIEW" && docker compose "$@"); }
else
  command -v sg > /dev/null 2>&1 || fail "docker access denied and sg is unavailable"
  compose() {
    local quoted
    quoted=$(printf '%q ' "$@")
    sg docker -c "cd $(printf '%q' "$PREVIEW") && docker compose $quoted"
  }
fi

if [ "${1:-}" = "--list" ]; then
  (cd "$HERE" && node run.js --list)
  exit 0
fi

[ -d "$PREVIEW/node_modules/puppeteer-core" ] || (cd "$PREVIEW" && npm install --silent)

# Chromium discovery, in shot.sh's preference order: an explicit CHROME=, a
# cached Playwright build, then common system binaries.
if [ -z "${CHROME:-}" ]; then
  CHROME="$(find "$HOME/.cache/ms-playwright" -path '*/chrome-linux64/chrome' -type f 2>/dev/null | head -1 || true)"
fi
if [ -z "${CHROME:-}" ]; then
  for candidate in chromium chromium-browser google-chrome google-chrome-stable; do
    if command -v "$candidate" > /dev/null 2>&1; then
      CHROME="$(command -v "$candidate")"
      break
    fi
  done
fi
[ -n "${CHROME:-}" ] && [ -x "$CHROME" ] || fail "no Chromium found; set CHROME=<path-to-chrome>"
export CHROME

# Every scene, or the ones named. A name is the PNG's own stem.
mapfile -t MANIFEST < <(cd "$HERE" && node run.js --list)
[ "${#MANIFEST[@]}" -gt 0 ] || fail "no scenes found under $HERE/scenes"

WANTED=("$@")
if [ "${#WANTED[@]}" -eq 0 ]; then
  for line in "${MANIFEST[@]}"; do
    WANTED+=("${line%%$'\t'*}")
  done
fi

manifest_line() {
  local name="$1" line
  for line in "${MANIFEST[@]}"; do
    if [ "${line%%$'\t'*}" = "$name" ]; then
      printf '%s\n' "$line"
      return 0
    fi
  done
  return 1
}

for name in "${WANTED[@]}"; do
  manifest_line "$name" > /dev/null || fail "no scene named $name (try --list)"
done

# --- The container ----------------------------------------------------------
# It has to be a screens container: the plain preview has neither the stand-in
# gh nor the transcript provider, and it binds with --no-tailscale. The marker
# file the entrypoint writes is how the two are told apart.
answering() { curl -fsS --max-time 3 "http://127.0.0.1:$PORT/api/v1/workspace" > /dev/null 2>&1; }
screens_mode() { compose exec -T dux test -f /data/screens-mode > /dev/null 2>&1; }

if answering && screens_mode; then
  echo ">> preview already serving on $PORT with the screenshot fixtures"
else
  echo ">> bringing the preview up with the screenshot fixtures"
  (cd "$PREVIEW" && ./up.sh)
  for _ in $(seq 1 60); do
    answering && break
    sleep 2
  done
  answering || fail "the preview never answered on port $PORT"
fi

# --- Sizes before -----------------------------------------------------------
declare -A OLD_SIZE
for name in "${WANTED[@]}"; do
  file="$SCREENS/$name.png"
  if [ -f "$file" ]; then
    OLD_SIZE["$name"]=$(stat -c%s "$file")
  else
    OLD_SIZE["$name"]=0
  fi
done

# --- Seed -------------------------------------------------------------------
echo ">> seeding the scene"
(cd "$HERE" && node seed.js)

# --- Shoot ------------------------------------------------------------------
WEB=()
TUI=()
for name in "${WANTED[@]}"; do
  IFS=$'\t' read -r -a fields <<< "$(manifest_line "$name")"
  if [ "${fields[1]}" = "tui" ]; then TUI+=("$name"); else WEB+=("$name"); fi
done

if [ "${#WEB[@]}" -gt 0 ]; then
  echo ">> shooting ${#WEB[@]} browser scene(s)"
  (cd "$HERE" && node run.js "${WEB[@]}")
fi

for name in "${TUI[@]}"; do
  IFS=$'\t' read -r -a fields <<< "$(manifest_line "$name")"
  pngfile="${fields[2]}"
  cols="${fields[3]}"
  rows="${fields[4]}"
  theme="${fields[5]}"
  crop="${fields[6]:-}"
  echo ">> shooting terminal UI scene $name (${cols}x${rows}, $theme)"
  args=("$HERE/scenes/$name.js" "$SCREENS/$pngfile" --cols "$cols" --rows "$rows" --theme "$theme")
  [ -n "$crop" ] && args+=(--crop "$crop")
  "$PREVIEW/tui-shot.sh" "${args[@]}"
  # tui-shot.sh writes three companions beside the PNG; the docs want the image
  # only, so the working artifacts do not land in website/public.
  rm -f "$SCREENS/$name.ansi" "$SCREENS/$name.txt" "$SCREENS/$name.json"
done

# --- The table --------------------------------------------------------------
printf '\n%-36s %10s %10s   %s\n' file old new change
for name in "${WANTED[@]}"; do
  file="$SCREENS/$name.png"
  old="${OLD_SIZE[$name]}"
  if [ -f "$file" ]; then
    new=$(stat -c%s "$file")
  else
    new=0
  fi
  if [ "$new" = "0" ]; then
    note="NOT WRITTEN"
  elif [ "$old" = "0" ]; then
    note="new"
  elif [ "$old" = "$new" ]; then
    note="same size"
  else
    note="changed"
  fi
  printf '%-36s %10s %10s   %s\n' "$name.png" "$old" "$new" "$note"
done
