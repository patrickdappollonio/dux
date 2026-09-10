#!/bin/sh
# Container entrypoint: seed an isolated dux config + a couple of demo repos on
# first run, then serve the web UI. All state lives under /data and /repos
# (named volumes), so nothing here touches the host's real ~/.config/dux.
set -e

export DUX_HOME=/data/dux
PORT="${DUX_PORT:-8790}"

# --- Seed a valid config on first boot -------------------------------------
# `dux config regenerate --yes` writes the full canonical default config (all
# four default providers) headlessly. We then append a fake streaming provider
# used to exercise the working-state visuals. Idempotent: only on first boot.
if [ ! -f "$DUX_HOME/config.toml" ]; then
  echo "entrypoint: generating fresh dux config at $DUX_HOME"
  dux config regenerate --yes
  cat >> "$DUX_HOME/config.toml" <<'EOF'

[providers.fake]
command = "/usr/local/bin/fake-agent"
args = []
EOF
fi

# --- Seed demo git repos (so the project picker has something to add) -------
git config --global init.defaultBranch main
git config --global user.email "dux@example.com"
git config --global user.name "dux preview"
git config --global --add safe.directory '*'
# Nothing in here can answer a credential prompt, and the container has a tty, so
# a git operation against an unreachable remote would sit on "Username for ..."
# forever and wedge whatever dux was doing. Fail fast instead.
git config --global credential.helper ""
export GIT_TERMINAL_PROMPT=0
export GIT_ASKPASS=/bin/true

for r in demo-api demo-web; do
  d="/repos/$r"
  if [ ! -d "$d/.git" ]; then
    echo "entrypoint: seeding repo $d"
    mkdir -p "$d/src"
    (
      cd "$d"
      git init -q
      printf '# %s\n\nSeed repo for dux preview.\n' "$r" > README.md
      printf 'def main():\n    print("hello from %s")\n' "$r" > src/main.py
      git add -A
      git commit -qm "Seed $r"
    )
  fi
done

# --- Screenshot fixtures (opt-in) ------------------------------------------
# The committed screenshot tool needs two stand-ins an ordinary preview must not
# have: a `gh` answering only dux's auth probe and its bounded `pr view`, so the
# pull-request chip, banner and from-PR dialog can be shot without a GitHub
# login, and a transcript provider standing in for `claude`, so the standalone
# agent shows a scripted session rather than a streaming fixture. Both are
# mounted read-only and installed only when this is asked for, and the marker
# file is how the host tells a screens container from a plain one.
if [ "${DUX_SCREENS:-0}" = "1" ] && [ -d /fixtures ]; then
  echo "entrypoint: installing screenshot fixtures (stand-in gh, transcript provider)"
  install -m 0755 /fixtures/gh /usr/local/bin/gh
  install -m 0755 /fixtures/notes-agent /usr/local/bin/claude
  # The tab strip shows one tab per provider, and dux refuses to open a tab for a
  # CLI that is not on PATH. opencode is not installed here, so it points at the
  # fake provider: that shot is about the strip, not about opencode.
  ln -sf /usr/local/bin/fake-agent /usr/local/bin/opencode
  : > /data/screens-mode
else
  rm -f /data/screens-mode
fi

# The Tailscale flag is a variable because the Preferences screenshot needs it
# gone: `--no-tailscale` refuses a live mode change for the whole run, and the
# dialog says so instead of carrying the general copy the docs describe.
if [ "${DUX_NO_TAILSCALE:-1}" = "1" ]; then
  set -- --no-tailscale
else
  set --
fi

echo "entrypoint: serving dux web UI on 0.0.0.0:$PORT (isolated, no login gate)"
exec dux server --bind "0.0.0.0:$PORT" "$@"
