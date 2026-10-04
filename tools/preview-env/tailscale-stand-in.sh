#!/bin/sh
# A stand-in `tailscale` CLI for containers that have no tailnet. It answers the
# three questions dux asks (`ip`, `status --json [--peers=false]` and
# `serve status --json`) from files, and refuses everything else, so dux sees a
# machine on a tailnet without one existing. The entrypoint installs it on PATH
# only when DUX_FAKE_TAILSCALE=1, and gives loopback the address it reports so
# the Tailscale listener really binds.
#
# The answers live in files rather than in this script so a journey can change
# them while dux runs (switch on a stand-in TCP forward, say) and dux reads the
# new answer at its next look, exactly as it would read the real CLI. Every name
# and address in them is obviously fake.
dir="${DUX_FAKE_TAILSCALE_DIR:-/data/tailscale}"
case "$*" in
  "ip") cat "$dir/ip" ;;
  "status --json --peers=false" | "status --json") cat "$dir/status.json" ;;
  "serve status --json") cat "$dir/serve.json" ;;
  *)
    echo "the stand-in tailscale only answers what dux asks, not: $*" >&2
    exit 1
    ;;
esac
