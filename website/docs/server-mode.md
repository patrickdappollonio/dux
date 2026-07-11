---
title: Server mode overview
description: What server mode is, how to start it (the dux server command or the in-app flip), the startup banner, the honest no-login trust model, and every [server] config key with its default.
group: Server mode
order: 60
---

Everything dux does in your terminal, it can also do in a browser. Server mode
serves a web UI of the very same workspace: the same projects, the same agents on
the same worktrees, the same live engine driving the same PTYs. Nothing is
mirrored or re-synced. You open a URL and you are looking straight at the running
engine, from your laptop, your phone, or a tablet on the couch.

That is the whole idea: **one workspace, many screens.** Start an agent from the
TUI at your desk, walk away, and pick the exact same session up on your phone. Two
browser tabs on two devices see the same terminal at the same moment. There is one
workspace and everyone pointed at it shares it.

## Two ways to start it

### `dux server`

Run the web UI directly, with no TUI in front of it:

```bash
dux server
```

By default it binds `127.0.0.1:8080` (loopback only) and prints a small
vite-style banner listing exactly what bound, one row per address with its
`http://…` URL, plus a reachability note. Open the URL and you are in.

It takes a few flags:

```text
dux server [OPTIONS]

  --bind <ADDR:PORT>   Bind this exact address, overriding [server] host+port.
                       An IP:port socket (hostnames are NOT resolved), e.g.
                       0.0.0.0:8080. May be given only once.
  --port <PORT>        Override [server] port only (ignored when --bind is set).
  --no-tailscale       Skip Tailscale detection this run.
  -h, --help           Print help and exit.
```

Precedence is easy to remember: `--bind` (an exact socket) wins over everything,
otherwise `--port` overrides just the port on top of the configured host,
otherwise the `[server] host` and `port` from your config apply. When Tailscale is
enabled, its address is appended as a best-effort extra leg. A required address
that cannot bind is fatal and says so, the Tailscale leg failing to bind is only a
warning and the server carries on.

#### Stopping it

`Ctrl-c` (or a `SIGTERM`) starts a graceful shutdown, not an instant kill. dux
drains open connections and sends `SIGTERM` to every running agent so its CLI
gets a chance to save state, waiting up to `[server] shutdown_timeout_seconds`
(30 seconds by default) before force-killing whatever is left. A second `Ctrl-c`
during that wait skips the grace period and exits immediately.

Only one `dux server` (or `dux` TUI) can run against a given config directory at
a time: both acquire the same single-instance lock, so starting a second one
against the same directory fails fast with a clear "already running" message
instead of two processes fighting over the same SQLite database. If you want
both the TUI and the browser open on the same live engine, use the in-app flip
below rather than starting a second process.

### Flip a running TUI into the browser

Already in the TUI with agents running and want a browser instead? Open the
command palette (`Ctrl-p`) and run **start-web-server**. It is a palette-only
command with no default keybinding, on purpose, because it is not something you
want to trigger by accident.

The flip is graceful. Your **agents keep running the entire time** (no relaunch,
no lost conversations), the live engine is simply handed to the web server
in-process. Your terminal turns into a themed dux status screen showing the serve
URLs and an activity panel. Press `q` or `Esc` there to drop back into the TUI
around the same still-running engine, so you can bounce between the two surfaces
as much as you like. `Ctrl-c` quits the whole process.

One difference worth filing away: **`dux server` honors your configured
`[server] host` and `--bind`, but the in-app flip always serves loopback plus
your Tailscale address only.** The flip never reaches for a custom host. If you
need to bind a specific interface, start with `dux server`.

## The trust model, stated plainly

dux is a single-tenant, trusted-access tool, and server mode is built around that
honestly: **there is no login. None.** No password, no token, no user accounts.
Access control is delegated to where you bind and who can reach it.

- **Loopback by default.** `127.0.0.1:8080` is reachable only from the machine
  dux runs on. Nothing leaves the box.
- **Tailscale, opt-out.** When `tailscale_enabled` is on (it is by default), dux
  also binds your machine's Tailscale address, so your own tailnet devices can
  reach it over WireGuard. Anyone on your tailnet can drive your agents, with no
  further gate, so treat your tailnet as trusted.
- **Anything wider is on you.** Binding a LAN or public address (say
  `--bind 0.0.0.0:8080`) puts your agents and worktrees in reach of anyone who
  can hit that address, with no login in front. dux prints a loud warning before
  it does this. Put it behind a trusted reverse proxy or keep it on Tailscale.

Two automatic defenses always run, and they are about browser attacks, not user
authentication: a **Host-header allowlist** (so a malicious page cannot
DNS-rebind your browser into the server) and a **same-origin check** on every
socket upgrade and every write request (so another site cannot ride your session).
A Tailscale `100.x` IP is allowed automatically, but a MagicDNS name like
`box.tailnet.ts.net` is not an IP literal, so if you reach dux by that name you
must add it to `allowed_hosts` (below) or the host guard will answer `403`.

The short version: **everyone who can reach the server shares one workspace.**
They can attach to any agent or terminal, browse the server's filesystem through
the project picker, run git actions, and see every session. That is intentional
for a per-developer or trusted-team tool. It is not a multi-user product, so do
not expose it to people you would not hand a terminal on that machine.

## The `[server]` config keys

Everything is configurable, and the config file is the documentation, so each key
below carries a full inline comment in your `config.toml`. Here is the shape,
with defaults:

```toml
[server]
# LOCAL MODE bind host for `dux server`. An IP literal only (hostnames are not
# resolved): 127.0.0.1 is the loopback default, 0.0.0.0 is all interfaces.
host = "127.0.0.1"

# LOCAL MODE port for `dux server` and the palette flip.
port = 8080

# Opt-out Tailscale binding. When true, also bind the machine's Tailscale
# address so tailnet devices can reach dux. If the tailscale CLI is missing or
# the daemon is down, dux warns and serves the configured host only.
tailscale_enabled = true

# Extra Host header values to accept when a request is not same-origin. List a
# reverse-proxy hostname or a tailnet MagicDNS name here so it is not rejected.
allowed_hosts = []
```

The rest tune presentation and limits:

| Key | Default | What it does |
|---|---|---|
| `color` | `"auto"` | Colored, vite-style console output for `dux server` (`auto`, `always`, `never`). |
| `access_log` | `true` | Print a per-request access log line to the `dux server` console (never to `dux.log`, so pipe stdout to capture it). `/healthz` is always skipped. |
| `title` | `"dux"` | Web-only instance name: the browser tab title and the wordmark in the projects pane. Set `"dux (prod)"` to tell tabs apart. |
| `favicon` | `""` | Web-only favicon tint so several dux tabs are distinguishable. Empty keeps the yellow duck; otherwise a curated color (violet, blue, sky, cyan, teal, green, amber, orange, red, pink, rose). |
| `shutdown_timeout_seconds` | `30` | Seconds the server waits for agents and terminals to save state after SIGTERM before force-killing. A second Ctrl-c during the wait exits immediately. |
| `max_websocket_events_connections` | `32` | Cap on the status/event sockets (one per browser tab). |
| `max_websocket_agent_connections` | `32` | Cap on agent-PTY sockets. |
| `max_websocket_terminal_connections` | `64` | Cap on companion-terminal PTY sockets. |
| `max_websocket_tab_connections` | `64` | Cap on extra-tab PTY sockets across all agents (its own pool, so many-tab agents cannot starve others). |
| `max_websocket_tabs_per_agent` | `8` | Per-agent fairness sub-quota on that tab pool. |

The connection caps exist so a runaway number of tabs cannot exhaust the server.
Going over a cap returns HTTP `503` until a slot frees. Setting a cap to `0`
permanently blocks that whole class of socket until restart, and the semaphores
are built at startup, so **changing any cap needs a server restart**, not just a
config reload. You can safely leave all of these alone unless you are running an
unusually busy instance. `title` and `favicon` are the two most people actually
touch, and you can set them live from the web itself (see
[The workspace in the browser](/docs/web-workspace)).

Server mode shares the rest of your config with the TUI. The `[capabilities]`
switches that bridge an agent's notifications and clipboard writes into the
browser are covered in [Terminal capabilities](/docs/terminal-capabilities), and
the general config file lives in [Configuration](/docs/configuration). One knob
worth calling out here: on a headless server there is no host terminal to
mirror, so `terminal_identity = "auto"` (the default) presents **ghostty** to
every newly launched agent, an identity the browser terminal renders well. See
[Terminal capabilities](/docs/terminal-capabilities) for the full story,
including how it differs from the TUI's own mirroring behavior.

### Editing config from the browser

You do not need shell access to the machine to change settings. The command
palette's **Edit config** command opens a raw Monaco TOML editor over your actual
`config.toml`, right in the page; saving writes the file but does not apply it
live, so run **reload-config** afterward to pick up the change. For just the
environment, **Configure global environment** opens a dedicated dialog for
workspace-wide environment variables that every project inherits, which any
project can still override with its own project-level environment settings.

## Where to go next

- [The workspace in the browser](/docs/web-workspace): the layout, the browser
  terminals, ownership and take-over, clipboard, and the mobile experience.
- [The code editor](/docs/web-editor): open and edit any file in a worktree with
  a real editor, right in the page.
- [Git without leaving the browser](/docs/web-git): stage, commit, push, pull,
  and review diffs.
- [Agents from the browser](/docs/web-agents): create, fork, adopt, and manage
  agents and their provider tabs.
