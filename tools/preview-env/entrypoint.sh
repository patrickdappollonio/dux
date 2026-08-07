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

echo "entrypoint: serving dux web UI on 0.0.0.0:$PORT (isolated, no login gate)"
exec dux server --bind "0.0.0.0:$PORT" --no-tailscale
