---
title: Server mode overview
description: What server mode is, the three ways to serve it (the dux server command, the in-app flip, or quietly behind a running TUI), the startup banner, the honest no-login trust model, and every [server] config key with its default.
group: Web UI
order: 60
---

dux has two front ends over one engine: a terminal UI and a web UI. Both are first
class, and both are staying. Server mode is the web one, and it serves the very
same workspace: the same projects, the same agents on the same worktrees, the same
live engine driving the same PTYs, and the same config file. Nothing is mirrored or
re-synced. You open a URL and you are looking straight at the running engine, from
your laptop, your phone, or a tablet on the couch. An agent you start in one front
end is the same agent in the other.

The two are not identical, on purpose. Each surface does what its medium is good
at. The terminal gives you full keyboard control, rebindable keys, a command
palette and themes. The browser gives you reach: any device on your network,
including a phone, plus editing files in the page and desktop notifications. Where
a capability only makes sense on one side, it lives on one side, and the page that
covers it says why. To know whether something is available where you are, the
surface itself is the answer: the terminal's help overlay and command palette list
what it can do, and the browser's cog menu and row menus list what it can do.

That is the whole idea: **one workspace, many screens.** As many browsers as you
like can be pointed at it at once, and two devices see the same terminal at the same
moment; there is one workspace and everyone pointed at it shares it. Start an agent
at your desk in the terminal UI, hand the workspace to the browser with the flip
below, walk away, and pick that exact session up on your phone with it still running.

You can have both front ends up at once, if you ask for it: one setting keeps a web
server serving quietly in the background of a running terminal UI. What you cannot
do is run two dux processes against one config directory, so all three ways of
serving below are three shapes of the same single process rather than three servers.
Pick the one that matches where you want to be sitting.

## Three ways to serve it

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
drains open connections and sends `SIGTERM` (plus `SIGHUP`, the classic
"terminal closed" signal that interactive shells actually honor) to every
running agent and terminal so each gets a chance to save state and bow out,
waiting up to `[server] shutdown_timeout_seconds`
(30 seconds by default) before force-killing whatever is left. A second `Ctrl-c`
during that wait skips the grace period and exits immediately.

Only one `dux server` (or `dux` TUI) can run against a given config directory at
a time: both acquire the same single-instance lock, so starting a second one
against the same directory fails fast with a clear "already running" message
instead of two processes fighting over the same SQLite database. One process, one
workspace. The two sections below are how that one process serves the browser
without you restarting anything: the flip hands the terminal over, and the
background mode keeps it.

#### Restarting it, with a tab still open

Restart dux while a browser tab is sitting there, and that tab keeps running the
interface the *old* server sent it. It reconnects happily and carries on
rendering the new server's data through the old page, which is fine right up
until the two disagree about something, at which point it is quietly wrong in a
place nobody thinks to look.

So the tab checks. It reads `GET /api/v1/build` when it loads and remembers the
answer, then reads it again the moment it gets back onto a socket. The response is
two fields:

```json
{ "version": "development", "process": "0d0f7a2c-4d1e-4f0a-9d55-6b1f2c3a4d5e" }
```

`version` is the same string shown under the logo, and `process` identifies that
particular *run* of the server, minted fresh each time it starts. If both still
match, the reconnect is the ordinary one: the tab refetches its state in place
and whatever you had open (a half-written commit message, an editor tab) is
untouched. If either has moved, dux **reloads the page** for you, no prompt. The
page is stale by definition at that point, so there is nothing in it worth
keeping.

The `process` half is what makes this work while you are hacking on dux:
`version` reads `development` for every build that is not a tagged release, so it
would never move between two `cargo run`s. A dropped Wi-Fi connection, by
contrast, returns to the same run of the same build and reconnects in place,
exactly as it always did.

### Flip a running TUI into the browser

Already in the TUI with agents running and want a browser instead? Open the
command palette and run **start-web-server**. It is a palette-only
command with no default keybinding, on purpose, because it is not something you
want to trigger by accident.

The flip is graceful. Your **agents keep running the entire time** (no relaunch,
no lost conversations), the live engine is simply handed to the web server
in-process. Your terminal turns into a themed dux status screen showing the serve
URLs and an activity panel. Press `q` or `Esc` there to drop back into the TUI
around the same still-running engine; that stops serving the web UI, so it is a
hand-back rather than a second window, and you can flip again whenever you like.
`Ctrl-c` quits the whole process. Your agents keep running through every one of
these transitions.

One difference worth filing away: **`dux server` honors your configured
`[server] host` and `--bind`, but the in-app flip always serves loopback plus
your Tailscale address only.** The flip never reaches for a custom host. If you
need to bind a specific interface, start with `dux server`.

### Serve in the background, and keep the TUI

The flip is a hand-off. This is the other answer: keep the terminal UI exactly
where it is and run the web server behind it, so the same workspace is on your
terminal and on your phone at the same moment. Turn it on with one key in
`config.toml`:

```toml
[server]
serve_while_tui = true
```

Off by default. You do not have to edit the file to change your mind, either: the
palette commands **start-background-server** and **stop-background-server** turn it
on and off while dux is running, and they save your choice back to config, so the
next start comes up the way you left it. Starting binds before anything else
happens, which means a busy port is a message on the status line and your TUI is
untouched. Stopping leaves every agent and terminal running; only the listener goes
away, and browsers that were connected report the connection closed.

It binds exactly the way the flip does: loopback plus your Tailscale address, never
a custom host. And it is the flip's alternative, not its companion, so
**start-web-server** while this is serving is refused with a note saying so, rather
than colliding with dux on its own port.

While it serves, the top bar grows a crumb right after the version: `● serving
:8080` on its own, becoming `● serving :8080 · 3 connected` once somebody is on it.
The address half is the standing reminder that a listener exists, which is why it is
the first crumb rather than the last: on a narrow terminal the things that fall off
the end should be the ones you can find elsewhere. The count is browser tabs, not
people, so one laptop with two tabs open is two, and a tab that vanished without
saying goodbye (a closed lid, dropped Wi-Fi) keeps counting until dux notices the
socket is dead.

Each agent's row picks up a quiet `2 remote` on its second line when browsers have
that agent open, counting every terminal it owns: its provider tabs and its
companion terminals, since the row is the only place that number appears. The center
pane's caption says the same for the provider tab it is showing.

**One driver at a time.** The terminal UI is a participant in the same
input-ownership model the browsers use, not a privileged owner of it. The first
device to type into a terminal nobody is driving claims it, and everybody who
arrives after that watches. Nothing passive ever takes it away again, which is the
point: arriving is not a gesture, so no amount of opening panes or reconnecting
moves the keyboard. Watching is real watching: live output, scrolling, copying, all
of it. When someone else has the
terminal you are looking at, the hint bar names the device that is driving and your
keystrokes do not reach the child, and **take-over-terminal** takes it back. In the
browser it is the take-over card on the pane, which names the terminal UI as `the
dux TUI` when that is what is driving. Taking a terminal over also retargets its
size to the device that took it, and everyone watching adopts that geometry, so one
terminal has one shape whoever is driving. Take-over works in both directions and it
is sticky either way: losing a terminal does not silently give it back to you, so
nothing swaps under either device's fingers.

Quitting the TUI stops the listener on the way out. What it does not do is decide
anything about next time: your saved setting is what decides that.

Which to reach for: `dux server` when nothing needs a terminal (a headless box, a
tmux pane you will detach from), the flip when you are done with the terminal for
now and want the browser to be the whole story, and this when you want to keep
working in the terminal and still be able to pick the same agent up on the couch.
The flip is unchanged by any of this, and it is still there whenever the background
mode is off.

## The trust model, stated plainly

dux is a single-tenant, trusted-access tool, and server mode is built around that
honestly: **there is no login. None.** No password, no token, no user accounts.
Access control is delegated to where you bind and who can reach it.

- **Loopback by default.** `127.0.0.1:8080` is reachable only from the machine
  dux runs on. Nothing leaves the box.
- **Tailscale, opt-out.** Unless `tailscale = "no"`, dux also binds your machine's
  Tailscale address, so your own tailnet devices can reach it over WireGuard.
  Anyone on your tailnet can drive your agents, with no further gate, so treat
  your tailnet as trusted. On the default `"auto"` this is a standing fact rather
  than a snapshot: dux binds that address whenever the interface is there, drops
  the listener when it goes, and binds it again when it comes back, all without a
  restart. See [Reaching dux over Tailscale](/docs/tailscale).
- **A background listener lasts as long as dux does.** `serve_while_tui = true`
  means there is a server running for the whole time your terminal UI is open, not
  only while you happen to be looking at a browser. Same trust model as the rest of
  this page; what changes is how much of your day it applies to.
- **Anything wider is on you.** Binding a LAN or public address (say
  `--bind 0.0.0.0:8080`) puts your agents and worktrees in reach of anyone who
  can hit that address, with no login in front. dux prints a loud warning before
  it does this. Put it behind a trusted reverse proxy or keep it on Tailscale.
  [Hosting dux behind a login](/docs/public-hosting) is one worked example of the
  proxy half: TLS, `oauth2-proxy` with GitHub, and dux on a private network.

Two automatic defenses always run, and they are about browser attacks, not user
authentication: a **Host-header allowlist** (so a malicious page cannot
DNS-rebind your browser into the server) and a **same-origin check** on every
socket upgrade and every write request (so another site cannot ride your session).
A Tailscale `100.x` IP is allowed automatically, whether or not that leg happens
to be bound at the moment, but a MagicDNS name like `box.tailnet.ts.net` is not an
IP literal, so if you reach dux by that name you must add it to `allowed_hosts`
(below) or the host guard will answer `403`.

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
# Bind host for `dux server`. An IP literal only (hostnames are not resolved):
# 127.0.0.1 is the loopback default, 0.0.0.0 is all interfaces. Serving from
# inside the TUI ignores this either way and always binds loopback (+ Tailscale).
host = "127.0.0.1"

# Bind port. Every way of serving uses it.
port = 8080

# Whether dux also binds the machine's Tailscale address, so tailnet devices can
# reach it. "auto" (the default) binds it whenever the interface exists and keeps
# watching, so the listener comes and goes with your tailnet connection; "yes"
# binds it once at startup and never looks again; "no" never binds it. If the
# tailscale CLI is missing or the daemon is down, dux warns and serves the
# configured host only.
tailscale = "auto"

# Serve the web UI in the background while the terminal UI keeps running, on
# loopback plus the Tailscale address, exactly like the palette flip binds. Off
# by default. The start-background-server and stop-background-server palette
# commands flip it while dux runs and save the choice back here. With this on,
# a listener exists for as long as dux does, and there is no login.
serve_while_tui = false

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
| `file_drop_max_bytes` | `104857600` | Largest single file you can drag, or image you can paste, onto a terminal or agent pane in the browser (100 MiB). A bigger file is refused and nothing is written. `0` switches file drop and image paste off. |
| `file_drop_max_concurrency` | `2` | How many dropped-file uploads are accepted at once. Bounds buffered upload memory, not just queued work. An upload beyond the limit waits up to 30 seconds for a slot, then is refused with a `503` rather than queueing indefinitely. `0` clamps to `1`. |

`host`, `port`, `allowed_hosts` and `tailscale` are read when serving starts, so
changing any of them needs a server restart and a config reload says so. That
includes `tailscale`, even though `"auto"` binds and unbinds by itself: the mode
decides whether dux watches the interface at all, and it is decided once.

`serve_while_tui` is the exception among the binding keys, and deliberately so: it
is a live switch. A
config reload that flips it acts on it there and then, in both directions, because
someone who edits the file to turn a listener off has asked for the listener to go
away, not to be told about it at the next restart.

The two `file_drop_*` keys are read at startup like the connection caps, so
changing either needs a server restart. The full story of what dropping or
pasting a file does, and where it lands, is in
[Dropping and pasting files onto an agent](/docs/dropping-files).

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

You do not need shell access to the machine to change settings. The cog menu's
**Configuration → Edit config file…** opens a raw Monaco TOML editor over your
actual `config.toml`, right in the page; saving writes the file but does not apply
it live, so run **Reload config** from the same submenu afterward to pick up the
change. For just the environment, **Global environment…** opens a dedicated dialog
for workspace-wide environment variables that every project inherits, which any
project can still override with its own project-level environment settings. For
the common `[ui]`/`[capabilities]` preferences you don't need the raw file at all:
they have rows in **Preferences…**.

## Where to go next

- [The workspace in the browser](/docs/web-workspace): the layout, the browser
  terminals, ownership and take-over, clipboard, and the mobile experience.
- [The code editor](/docs/web-editor): open and edit any file in a worktree with
  a real editor, right in the page.
- [Git without leaving the browser](/docs/web-git): stage, commit, push, pull,
  and review diffs.
- [Agents from the browser](/docs/web-agents): create, fork, adopt, and manage
  agents and their provider tabs.
- [Reaching dux over Tailscale](/docs/tailscale): how the tailnet address is found
  and bound, why a MagicDNS name needs `allowed_hosts`, and what plain HTTP costs
  you in the browser.
- [Hosting dux behind a login](/docs/public-hosting): a reverse proxy plus
  `oauth2-proxy` with GitHub in one Compose file, for when the server is not on a
  private network.
