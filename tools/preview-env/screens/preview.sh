# Shared by reshoot.sh and ../checks/run.sh: the compose command and the
# Chromium they drive. Sourced, not run; the sourcing script has set PREVIEW (the
# tools/preview-env directory) and a fail() of its own.

# Docker access, resolved the way up.sh resolves it.
if docker info > /dev/null 2>&1; then
  compose() { (cd "$PREVIEW" && docker compose "$@"); }
else
  command -v sg > /dev/null 2>&1 || fail "docker access denied and sg is unavailable"
  compose() {
    local quoted project=""
    quoted=$(printf '%q ' "$@")
    # The project is what tells this stack's container from another one's, so
    # it is carried into the sub-shell explicitly, and only when set, so an
    # unset project keeps compose's own default rather than an empty string.
    if [ -n "${COMPOSE_PROJECT_NAME:-}" ]; then
      project="export COMPOSE_PROJECT_NAME=$(printf '%q' "$COMPOSE_PROJECT_NAME") && "
    fi
    sg docker -c "cd $(printf '%q' "$PREVIEW") && ${project}docker compose $quoted"
  }
fi

# The Chromium the browser runs drive, and puppeteer to drive it with.
prepare_browser() {
  [ -d "$PREVIEW/node_modules/puppeteer-core" ] || (cd "$PREVIEW" && npm install --silent)

  # Chromium discovery: an explicit CHROME=, a cached Playwright build, then
  # common system binaries, in shot.sh's order with Brave added last.
  if [ -z "${CHROME:-}" ]; then
    CHROME="$(find "$HOME/.cache/ms-playwright" -path '*/chrome-linux64/chrome' -type f 2>/dev/null | head -1 || true)"
  fi
  if [ -z "${CHROME:-}" ]; then
    for candidate in chromium chromium-browser google-chrome google-chrome-stable brave-origin; do
      if command -v "$candidate" > /dev/null 2>&1; then
        CHROME="$(command -v "$candidate")"
        break
      fi
    done
  fi
  [ -n "${CHROME:-}" ] && [ -x "$CHROME" ] || fail "no Chromium found; set CHROME=<path-to-chrome>"
  export CHROME
}
