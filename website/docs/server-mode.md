---
title: Server mode overview
description: The three ways to serve the web UI, the startup banner, the no-login trust model, and every [server] config key with its default.
group: Web UI
order: 60
---

Server mode is dux in a browser. It serves the same workspace the terminal UI serves:
the same projects, the same agents on the same worktrees, the same live engine driving
the same PTYs, the same config file. Nothing is mirrored or re-synced. An agent you
start in one front end is the same agent in the other.

Both front ends are first class, and they differ on purpose:

- The **terminal** gives you full keyboard control, rebindable keys, a command palette,
  and themes.
- The **browser** gives you reach: any device on your network, a phone included, plus
  editing files in the page and desktop notifications.

To know whether something is available where you are, ask the surface: the terminal's
help overlay and command palette list what it can do, and the browser's cog menu and
row menus list what it can do.

Point as many browsers at it as you like. Two devices see the same terminal at the same
moment.

> [!IMPORTANT]
> You cannot run two dux processes against one config directory. The three ways to serve
> below are three shapes of one process, not three servers.

## Three ways to serve it

### `dux server`

Run the web UI with no TUI in front of it:

```bash
dux server
```

It binds `127.0.0.1:8080` (loopback only) by default and prints a small vite-style
banner: one row per bound address with its `http://…` URL, plus a reachability note.

The flags:

```text
dux server [OPTIONS]

  --bind <ADDR:PORT>   Bind this exact address, overriding [server] host+port.
                       An IP:port socket (hostnames are NOT resolved), e.g.
                       0.0.0.0:8080. May be given only once.
  --port <PORT>        Override [server] port only (ignored when --bind is set).
  --no-tailscale       Skip Tailscale detection this run.
  -h, --help           Print help and exit.
```

Precedence: `--bind` wins over everything, then `--port` overrides just the port on top
of the configured host, then `[server] host` and `port` from your config. When Tailscale
is enabled its address is appended as a best-effort extra leg. A required address that
cannot bind is fatal. The Tailscale leg failing to bind is only a warning, and the
server carries on.

#### Stopping it

`Ctrl-c` (or a `SIGTERM`) starts a graceful shutdown. dux drains open connections and
sends `SIGTERM` and `SIGHUP` to every running agent and terminal so each can save state,
waiting up to `[server] shutdown_timeout_seconds` (30 seconds by default) before
force-killing whatever is left. A second `Ctrl-c` during that wait exits immediately.

Only one `dux server` (or `dux` TUI) can run against a given config directory. Both take
the same single-instance lock, so a second one fails fast with an "already running"
message rather than two processes fighting over the same SQLite database.

#### Restarting it, with a tab still open

Restart the server and any open browser tab notices and reloads itself, with no prompt.
Anything held only in the page, an unsaved editor draft most of all, goes with it, so
finish what you are typing first.

A dropped Wi-Fi connection is not a restart: the tab reconnects in place and keeps
everything it had open, for as long as you leave the page open. See
[the browser terminals](/docs/web-workspace#the-browser-terminals) for exactly how long
it keeps trying and what it does when your phone falls asleep.

### Flip a running TUI into the browser

Already in the TUI and want a browser instead? Open the command palette and run
**start-web-server**. It is palette-only with no default keybinding, so you cannot
trigger it by accident.

Your **agents keep running the entire time**: no relaunch, no lost conversations. The
live engine is handed to the web server in-process. Your terminal turns into a themed
dux status screen showing the serve URLs and an activity panel. Press `q` or `Esc` there
to drop back into the TUI around the same still-running engine, which stops serving the
web UI; you can flip again whenever you like. `Ctrl-c` quits the whole process.

> [!IMPORTANT]
> `dux server` honors your configured `[server] host` and `--bind`. The in-app flip
> always serves loopback plus your Tailscale address only. To bind a specific interface,
> start with `dux server`.

### Serve in the background, and keep the TUI

Keep the terminal UI where it is and run the web server behind it, so the same workspace
is on your terminal and on your phone at once:

```toml
[server]
serve_while_tui = true
```

Off by default. The palette commands **start-background-server** and
**stop-background-server** turn it on and off while dux runs, and save your choice back
to config.

Starting binds before anything else happens, so a busy port is a message on the status
line and your TUI is untouched. Stopping leaves every agent and terminal running: only
the listener goes away, and connected browsers report the connection closed. Quitting
the TUI stops the listener too, and changes nothing about your saved setting.

**set-tailscale-mode** in the same palette changes whether the Tailscale leg exists,
without stopping anything: see [Changing the mode while dux is serving](/docs/tailscale#changing-the-mode-while-dux-is-serving).

It binds exactly the way the flip does: loopback plus your Tailscale address, never a
custom host.

> [!IMPORTANT]
> The background mode is the flip's alternative, not its companion. Running
> **start-web-server** while the background server serves is refused with a note saying
> so.

While it serves, the top bar grows a crumb right after the version: `● serving :8080` on
its own, becoming `● serving :8080 · 3 connected` once somebody is on it. The count is
browser tabs, not people, so one laptop with two tabs open counts as two, and a tab that
vanished without saying goodbye keeps counting until dux notices the socket is dead.

Each agent's row picks up a quiet `2 remote` on its second line when browsers have that
agent open, counting every terminal it owns: its provider tabs and its companion
terminals. The center pane's caption says the same for the provider tab it is showing.

**One driver at a time.** The terminal UI is an ordinary participant in the same
input-ownership model the browsers use. One device drives a terminal and everybody else
watches, with live output, scrolling and copying. Nothing passive ever takes a terminal
away, and nothing passive ever gives one back.

Whenever the terminal you are looking at is not yours to type into, a card covers it, in
the terminal UI and in the browser alike, and it says which of the two things is true.
When another device is driving, the card names it. When nobody is driving, the card says
**Take control**; press it once and the terminal is yours. Either way the card carries
one button, **Take over**, and two ways to press it: click it, or, with the pane focused,
use the key that focuses an agent (Enter unless you have rebound it). Typing does not
claim a terminal, so keys pressed under the card go nowhere. An agent you start yourself
is yours straight away, with no card over it; agents dux reopens for you at startup wear
the **Take control** card until you press it. The card calls the terminal UI `the dux
TUI` when that is what has the keyboard. Taking a terminal over also retargets its size
to the device that took it, and everyone watching adopts that geometry. Take-over works
in both directions and is sticky either way: losing a terminal does not silently give it
back to you.

> [!NOTE]
> The card covers the terminal only. The tabs above it, the pull-request banner and the
> rest of the screen keep working, so you can move to another agent or another tab
> without taking anything over.

Which to reach for:

- **`dux server`** when nothing needs a terminal: a headless box, a tmux pane you will
  detach from.
- **The flip** when you are done with the terminal and want the browser to be the whole
  story.
- **The background mode** when you want to keep working in the terminal and still pick
  the same agent up on the couch.

## The trust model, stated plainly

dux is a single-tenant, trusted-access tool.

> [!WARNING]
> **There is no login. None.** No password, no token, no user accounts, and nothing to
> turn on in config. Access control is delegated entirely to where you bind and who can
> reach it.

> [!CAUTION]
> **Everyone who can reach the server shares one workspace.** They can attach to any
> agent or terminal, browse the server's filesystem through the project picker, run git
> actions, and see every session. Do not expose it to people you would not hand a
> terminal on that machine.

Where dux binds:

- **Loopback by default.** `127.0.0.1:8080` is reachable only from the machine dux runs
  on.
- **Tailscale, opt-out.** Unless `tailscale = "no"`, dux also binds your machine's
  Tailscale address, so your tailnet devices can reach it over WireGuard with no further
  gate. On the default `"auto"` dux binds that address whenever the interface is there,
  drops the listener when it goes, and binds it again when it comes back, with no
  restart. See [Reaching dux over Tailscale](/docs/tailscale).
- **A background listener lasts as long as dux does.** `serve_while_tui = true` means a
  server runs for the whole time your terminal UI is open.

> [!CAUTION]
> **Anything wider is on you.** Binding a LAN or public address (say
> `--bind 0.0.0.0:8080`) puts your agents and worktrees in reach of anyone who can hit
> that address, with no login in front. dux prints a loud warning before it does this.
> Put it behind a trusted reverse proxy or keep it on Tailscale.
> [Hosting dux behind a login](/docs/public-hosting) is one worked example: TLS,
> `oauth2-proxy` with GitHub, and dux on a private network.

Two automatic defenses always run. They are about browser attacks, not user
authentication:

- A **Host-header allowlist**, so a malicious page cannot DNS-rebind your browser into
  the server.
- A **same-origin check** on every socket upgrade and every write request, so another
  site cannot ride your session.

A Tailscale `100.x` IP is allowed automatically, whether or not that leg is bound at the
moment. A MagicDNS name like `box.tailnet.ts.net` is not an IP literal, so if you reach
dux by that name you must add it to `allowed_hosts` or the host guard answers `403`.

## The `[server]` config keys

Every key below carries a full inline comment in your `config.toml`:

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
# binds it once and then stops looking; "no" never binds it. If the
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
| `color` | `"auto"` | Colored, vite-style console output for `dux server` (`auto`, `always`, `never`). Read at startup. |
| `access_log` | `true` | Print a per-request access log line to the `dux server` console (never to `dux.log`, so pipe stdout to capture it). `/healthz` is always skipped. A config reload applies it. |
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
| `search_index_max_files` | `50000` | Cap on the web editor's "Search files…" flat walk. `0` disables the cap. A config reload applies it. |
| `replay_wait_seconds` | `8` | How long a browser waits for the terminal's screen to arrive after connecting before it stops waiting quietly and offers a Reconnect button. Counted in time the page is actually on screen, so a phone in your pocket does not burn through it. `0` disables the wait, leaving a slow screen covered indefinitely. A config reload applies it. |
| `reconnect_backoff_cap_seconds` | `10` | The longest gap a browser leaves between automatic reconnect attempts. It starts at half a second and widens up to this. A visible tab never gives up; raise this to be gentler on a struggling server, lower it to come back faster. A config reload applies it. |
| `heartbeat_seconds` | `15` | How often a visible browser tab checks its terminal connection is really alive. A Wi-Fi to cellular handoff can leave a connection that looks open and answers nothing, and this is what notices. A config reload applies it. |
| `heartbeat_deadline_seconds` | `30` | How long the browser waits for the answer to that check before deciding the connection is dead and reconnecting. Counted in time the page is on screen. Must be comfortably larger than `heartbeat_seconds`, or a slow network reconnects you needlessly; a value at or below it would reconnect over and over, so dux quietly uses twice `heartbeat_seconds` instead. A config reload applies it. |
| `pty_send_timeout_seconds` | `60` | How long dux waits for the first two things it sends a browser terminal, the handshake and the screen redraw, to actually arrive, before it gives up on that connection and lets the browser try again. A send finishes when the bytes get there, so on a slow connection this is really a measure of speed, and the screen redraw can be your whole scrollback. Set it too low and a phone on a bad signal can never finish attaching. A config reload applies it to the next terminal connection. |
| `tree_list_max_concurrency` | `8` | How many editor directory listings run at once. `0` disables the bound. Read at startup. |
| `release_notes_max_concurrency` | `2` | How many release-notes fetches run at once. `0` disables the bound. Read at startup. |

> [!IMPORTANT]
> Most of these are read once, when serving starts, so changing them needs a **server
> restart**, not just a config reload: `host`, `port`, `allowed_hosts`,
> both `file_drop_*` keys, every connection cap, and the two `*_max_concurrency`
> limits. A config reload says so for all of them, on either surface: the browser
> and the terminal app each warn you.
>
> `color` is read once too, but only by `dux server`, which is the only way of
> serving that prints a console. A reload that changes it says so in the browser
> and tells you it applies the next time you start `dux server`; the terminal app
> stays quiet, because nothing it can start reads the setting.
>
> The exceptions are `access_log`, `search_index_max_files`, `pty_send_timeout_seconds`
> and the four reconnect timings (`replay_wait_seconds`,
> `reconnect_backoff_cap_seconds`, `heartbeat_seconds` and
> `heartbeat_deadline_seconds`), which a reload applies to a running server. The four
> timings describe what the BROWSER does, and an open tab picks them up on its own
> within a moment of the reload; you do not have to refresh the page.
> `pty_send_timeout_seconds` applies to the next terminal you open.

`serve_while_tui` and `tailscale` are the two binding keys that are live switches: a
config reload that flips either acts on it there and then, in both directions.
`tailscale` can also be changed without touching the file at all, from the TUI palette
or the browser's Preferences dialog: see [Changing the mode while dux is serving](/docs/tailscale#changing-the-mode-while-dux-is-serving).

Going over a connection cap returns HTTP `503` until a slot frees. Setting a cap to `0`
blocks that whole class of socket until restart. Leave the caps alone unless you are
running an unusually busy instance. `title` and `favicon` you can set live from the web
itself (see [The workspace in the browser](/docs/web-workspace)).

What dropping or pasting a file does, and where it lands, is in
[Dropping and pasting files onto an agent](/docs/dropping-files).

Server mode shares the rest of your config with the TUI. The `[capabilities]` switches
that bridge an agent's notifications and clipboard writes into the browser are covered in
[Terminal capabilities](/docs/terminal-capabilities), and the general config file lives
in [Configuration](/docs/configuration).

> [!NOTE]
> On a headless server there is no host terminal to mirror, so
> `terminal_identity = "auto"` (the default) presents **ghostty** to every newly launched
> agent, an identity the browser terminal renders well. See
> [Terminal capabilities](/docs/terminal-capabilities) for how it differs from the TUI.

### Editing config from the browser

You do not need shell access to the machine to change settings:

![The Settings dialog scrolled to the row that chooses whether dux binds your Tailscale address.](/screens/preferences-dialog.png)

- **Configuration → Edit config file…** in the cog menu opens a raw Monaco TOML editor
  over your actual `config.toml`. Saving writes the file but does not apply it live, so
  run **Reload config** from the same submenu afterward.
- **Global environment…** opens a dialog for workspace-wide environment variables that
  every project inherits, which any project can override with its own project-level
  environment settings.
- The common `[ui]` and `[capabilities]` preferences have rows in **Preferences…**.

## Where to go next

- [The workspace in the browser](/docs/web-workspace): the layout, the browser
  terminals, ownership and take-over, clipboard, and the mobile experience.
- [The code editor](/docs/web-editor): open and edit any file in a worktree with a real
  editor, right in the page.
- [Git without leaving the browser](/docs/web-git): stage, commit, push, pull, and
  review diffs.
- [Agents from the browser](/docs/web-agents): create, fork, adopt, and manage agents
  and their provider tabs.
- [Reaching dux over Tailscale](/docs/tailscale): how the tailnet address is found and
  bound, why a MagicDNS name needs `allowed_hosts`, and what plain HTTP costs you.
- [Hosting dux behind a login](/docs/public-hosting): a reverse proxy plus
  `oauth2-proxy` with GitHub in one Compose file.
