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

# --- Journey seed hooks (opt-in) -------------------------------------------
# The container journey suite (crates/dux-journeys) copies shell hooks into
# /journey/seed.d before the container starts, to shape the config a journey
# needs (a port, a setting, a password set through `dux config set`). They run
# on every start, after the config above exists and before dux does, in name
# order; a hook that fails stops the container, loudly.
if [ -d /journey/seed.d ]; then
  for hook in /journey/seed.d/*.sh; do
    [ -f "$hook" ] || continue
    echo "entrypoint: running seed hook $hook"
    sh "$hook"
  done
fi

# --- A stand-in Tailscale (opt-in) -----------------------------------------
# DUX_FAKE_TAILSCALE=1 puts tailscale-stand-in on PATH, answering from files
# under /data/tailscale that a journey may rewrite while dux runs, and gives
# loopback the address it reports so the Tailscale listener really binds. That
# second step needs the NET_ADMIN capability, inside this container's own
# network namespace; without it the start fails rather than serving a tailnet
# leg that cannot exist. Every name and address is obviously fake.
if [ "${DUX_FAKE_TAILSCALE:-0}" = "1" ]; then
  ts=/data/tailscale
  mkdir -p "$ts"
  [ -f "$ts/ip" ] || echo "100.101.102.103" > "$ts/ip"
  ts_ip="$(cat "$ts/ip")"
  if [ ! -f "$ts/status.json" ]; then
    cat > "$ts/status.json" <<EOF
{"BackendState":"Running","TailscaleIPs":["$ts_ip"],"Self":{"HostName":"journey-box","DNSName":"journey-box.example-tailnet.ts.net.","TailscaleIPs":["$ts_ip"],"Online":true},"MagicDNSSuffix":"example-tailnet.ts.net","CurrentTailnet":{"Name":"example-tailnet","MagicDNSSuffix":"example-tailnet.ts.net","MagicDNSEnabled":true},"CertDomains":["journey-box.example-tailnet.ts.net"]}
EOF
  fi
  [ -f "$ts/serve.json" ] || echo "{}" > "$ts/serve.json"
  install -m 0755 /usr/local/share/dux-preview/tailscale-stand-in /usr/local/bin/tailscale
  ip addr show dev lo | grep -q "inet $ts_ip/" || ip addr add "$ts_ip/32" dev lo
  # A second tailnet address for the stand-in PEER, so a relayed tailnet
  # connection comes from somebody other than dux itself.
  peer_ip="${DUX_TAILNET_PEER_IP:-100.101.102.104}"
  ip addr show dev lo | grep -q "inet $peer_ip/" || ip addr add "$peer_ip/32" dev lo
  echo "entrypoint: stand-in tailscale answering with $ts_ip (peer $peer_ip)"
  DUX_NO_TAILSCALE=0
fi

# DUX_TAIL_LOG=1 copies dux.log onto the container's own output, so a journey
# that fails prints what dux logged, and a journey can wait for a log line.
if [ "${DUX_TAIL_LOG:-0}" = "1" ]; then
  touch "$DUX_HOME/dux.log"
  tail -n +1 -F "$DUX_HOME/dux.log" 2> /dev/null | sed -u 's/^/dux.log: /' &
fi

# --- Port relays (opt-in) ---------------------------------------------------
# DUX_RELAYS="4100=100.101.102.103:3890 4200=127.0.0.1:3890" listens on each
# published port and relays the raw TCP stream to the address after `=`, from
# inside this container. A relay onto loopback is a headerless forward (what
# `tailscale serve` with a TCP forward, or any port forwarder, looks like to
# dux); a relay onto the stand-in Tailscale address arrives on the Tailscale
# listener the way a tailnet peer's connection does.
for relay in ${DUX_RELAYS:-}; do
  listen="${relay%%=*}"
  target="${relay#*=}"
  echo "entrypoint: relaying port $listen to $target"
  socat "TCP-LISTEN:$listen,fork,reuseaddr" "TCP:$target" &
done

# The Tailscale flag is a variable because the Preferences screenshot needs it
# gone: `--no-tailscale` refuses a live mode change for the whole run, and the
# dialog says so instead of carrying the general copy the docs describe.
if [ "${DUX_NO_TAILSCALE:-1}" = "1" ]; then
  set -- --no-tailscale
else
  set --
fi

# Where dux listens. The preview binds every interface, which is what makes it
# reachable through the published port. DUX_BIND=local serves the way an
# unconfigured dux does instead (loopback, plus the Tailscale leg when there is
# one) on $PORT, so a journey can tell this machine and a tailnet peer apart.
if [ "${DUX_BIND:-}" = "local" ]; then
  set -- --port "$PORT" "$@"
  where="loopback (and Tailscale, if any) port $PORT"
else
  set -- --bind "${DUX_BIND:-0.0.0.0:$PORT}" "$@"
  where="${DUX_BIND:-0.0.0.0:$PORT}"
fi

# DUX_LAUNCH=tui runs the terminal UI in a detached tmux session instead of
# `dux server`, for the journeys that serve from the TUI (the flip, or
# `[server] serve_while_tui`); the container lives as long as that session does,
# and a journey drives it with `tmux -L journey send-keys`.
if [ "${DUX_LAUNCH:-server}" = "tui" ]; then
  echo "entrypoint: running the dux terminal UI in tmux session 'journey'"
  tmux -L journey new-session -d -s journey -x 160 -y 45 \
    -e "DUX_HOME=$DUX_HOME" -e TERM=xterm-256color dux
  while tmux -L journey has-session -t journey 2> /dev/null; do sleep 1; done
  echo "entrypoint: the dux terminal UI exited"
  exit 0
fi

# DUX_RESTART_LOOP=1 starts dux again whenever it exits, so a journey can stop
# the process inside this container and see a NEW run come up behind the same
# published port and the same state, the way a quick restart looks to an open tab.
# While /tmp/dux-hold exists the loop waits before starting the next run, so a
# journey can keep dux down for as long as it needs.
if [ "${DUX_RESTART_LOOP:-0}" = "1" ]; then
  while :; do
    echo "entrypoint: serving dux web UI on $where (restart loop)"
    dux server "$@" || true
    sleep 0.2
    while [ -f /tmp/dux-hold ]; do sleep 0.2; done
  done
fi

echo "entrypoint: serving dux web UI on $where (isolated)"
exec dux server "$@"
