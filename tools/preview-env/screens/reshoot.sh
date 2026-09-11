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

# Exported, not just computed: this is derived from DUX_SRC, and without it
# run.js writes beside its own file instead, so a run against another checkout
# rewrites the pictures of the one the tool lives in.
export SCREENS_DIR="$SCREENS"
export DUX_PORT="$PORT"
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

# Resolved in the browser branch below rather than up front. The terminal UI
# path needs a Chromium of its own (tui-shot.sh rasterizes the captured cells
# with one) but it hunts for it itself and says its own thing when it cannot find
# one, so gating every run here would just be this script refusing on another
# script's behalf, before the table that says what was rewritten.
prepare_browser() {
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
}

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

WEB=()
TUI=()
for name in "${WANTED[@]}"; do
  IFS=$'\t' read -r -a fields <<< "$(manifest_line "$name")"
  if [ "${fields[1]}" = "tui" ]; then TUI+=("$name"); else WEB+=("$name"); fi
done

# A scene that fails must not take the table with it: the table is how a person
# sees which pictures were rewritten and which were not, and that is most worth
# having on the run that went wrong. So failures are counted here and the script
# exits on the count at the very end.
FAILED=0

# The scenes that wrote nothing, one "<verdict> <name>" per line. Such a scene
# leaves the committed PNG exactly as it was, which from the outside is
# indistinguishable from a scene that came back identical, so the table reads
# this rather than guessing from the file size. The two verdicts are different
# news: "refused" is a guard saying the picture would have been wrong, "failed"
# is the tool not getting there at all.
REFUSED_FILE=$(mktemp)
trap 'rm -f "$REFUSED_FILE"' EXIT
export SCREENS_REFUSED_FILE="$REFUSED_FILE"

verdict_of() {
  local line
  line=$(grep -E "^(refused|failed) $1\$" "$REFUSED_FILE" 2> /dev/null | head -1)
  [ -n "$line" ] || return 1
  printf '%s\n' "${line%% *}"
}

# --- The browser scenes -----------------------------------------------------
# Only these need the long-running preview. A terminal UI journey brings up a
# disposable container of its own, so a reshoot of one of those touches neither
# the preview nor its seeded workspace.
if [ "${#WEB[@]}" -gt 0 ]; then
  prepare_browser
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

  echo ">> seeding the scene"
  (cd "$HERE" && node seed.js)

  echo ">> shooting ${#WEB[@]} browser scene(s)"
  if ! (cd "$HERE" && node run.js "${WEB[@]}"); then
    echo ">> some browser scenes wrote nothing; the table says which and why" >&2
    FAILED=$((FAILED + 1))
  fi
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
  # stdin closed: the capture runs `docker compose run`, which would otherwise
  # read from this script's own input.
  # 65 is the capture refusing itself: the cells do not say what the scene said
  # they must, or the picture came back empty. Anything else non-zero is the
  # capture never getting that far.
  status=0
  "$PREVIEW/tui-shot.sh" "${args[@]}" < /dev/null || status=$?
  if [ "$status" -ne 0 ]; then
    if [ "$status" -eq 65 ]; then
      echo ">> $name refused" >&2
      printf 'refused %s\n' "$name" >> "$REFUSED_FILE"
    else
      echo ">> $name failed" >&2
      printf 'failed %s\n' "$name" >> "$REFUSED_FILE"
    fi
    FAILED=$((FAILED + 1))
  fi
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
  if said=$(verdict_of "$name"); then
    if [ "$said" = "refused" ]; then
      note="refused; a guard said the picture was wrong, the committed one is kept"
    else
      note="failed; the tool did not get there, the committed one is kept"
    fi
  elif [ "$new" = "0" ]; then
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

if [ "$FAILED" -gt 0 ]; then
  echo >&2
  fail "$FAILED scene group(s) wrote nothing; the table above says which pictures were rewritten and why the rest were not"
fi
