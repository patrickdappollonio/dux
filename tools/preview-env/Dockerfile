# Isolated dux web and TUI preview container.
#
# The dux binary is NOT built here. It is built on the host (fast incremental
# cargo, and build.rs already has the host's node/npm set up to embed web/dist)
# and bind-mounted at runtime to /usr/local/bin/dux. This image only carries the
# RUNTIME deps: a recent glibc, git, node/npm, tmux, and the two agent CLIs.
#
# Base image note: the host is Arch-based (CachyOS, glibc 2.43). archlinux:latest
# is rolling, so its glibc tracks the host closely and the host-built binary runs
# without ABI skew. If a "GLIBC_2.xx not found" ever appears, switch the base:
#   BASE_IMAGE=cachyos/cachyos ./up.sh   (byte-identical to the host), or build
# dux in-container (see README fallback). Same machine, so CPU features match.
ARG BASE_IMAGE=archlinux:latest
FROM ${BASE_IMAGE}

# Refresh to current packages (brings glibc to the rolling version) and install
# the runtime toolchain. git supports worktree ops; node/npm run the agent CLIs;
# tmux provides a fixed terminal grid for TUI captures. `which` is required, not
# incidental: dux probes for the gh CLI by running `which gh`, and the base image
# does not ship it, so without this every GitHub feature reports gh as missing
# however it was installed. `iproute2` lets a TUI capture give the container's
# loopback a stand-in Tailscale address. Clean the package cache.
RUN pacman -Syu --noconfirm git nodejs npm ca-certificates tmux which iproute2 \
    && pacman -Scc --noconfirm

# The two default resume-capable providers, installed exactly as a user would
# (npm global). They land on PATH as `claude` and `codex`; dux's default config
# references them by those bare names. They run headless and prompt for login on
# first spawn, which is a valid, observable state for UI testing.
#
# AGENT_CLIS=0 leaves them out: the container journey suite (crates/dux-journeys)
# builds this same image with only the fake provider, and the two CLIs are the
# slowest and largest layer here.
ARG AGENT_CLIS=1
RUN if [ "$AGENT_CLIS" = "1" ]; then \
      npm install -g @anthropic-ai/claude-code @openai/codex \
      && npm cache clean --force; \
    fi

# The journey suite's extra tools, off for an ordinary preview. `socat` relays a
# published port onto an address inside the container (the stand-in tailnet
# address, or loopback with no forwarding header, the way `tailscale serve` with
# a raw TCP forward reaches dux). `curl` is how a journey makes a request FROM
# this machine, which no published port can do. `chromium` ships `chromedriver`,
# which is the browser the browser journeys drive over WebDriver.
ARG JOURNEY_TOOLS=0
RUN if [ "$JOURNEY_TOOLS" = "1" ]; then \
      pacman -Syu --noconfirm --needed socat curl chromium \
      && pacman -Scc --noconfirm; \
    fi

COPY fake-agent.sh /usr/local/bin/fake-agent
COPY entrypoint.sh /usr/local/bin/entrypoint
COPY tailscale-stand-in.sh /usr/local/share/dux-preview/tailscale-stand-in
COPY tui-driver.js /usr/local/bin/tui-driver.js
COPY tui-journey.example.js /usr/local/share/dux-preview/tui-journey.example.js
RUN chmod +x /usr/local/bin/fake-agent /usr/local/bin/entrypoint

# dux binary is bind-mounted read-only at /usr/local/bin/dux by compose.
EXPOSE 8790
ENTRYPOINT ["/usr/local/bin/entrypoint"]
