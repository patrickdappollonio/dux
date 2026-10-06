---
title: Server mode overview
description: The three ways to serve the web UI, the startup banner, the optional password and the trust model, and every [server] config key with its default.
group: Web UI
order: 60
---

Server mode is dux in a browser. It serves the same workspace the terminal UI serves:
the same projects, the same agents on the same worktrees, the same dux driving the same
terminals live, the same config file. Nothing is mirrored or re-synced. An agent you
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

It binds `127.0.0.1:3890` (loopback only) by default and prints a small vite-style
banner: one row per bound address with its `http://…` URL, plus a reachability note.
When stdout goes to a file or a pipe while stderr is still your terminal, every warning
and error it prints, and the shutdown's progress, also goes to stderr, so
`dux server > access.log` never hides a warning or leaves your terminal silent while it
stops. When both go to the same place (`> log 2>&1`, `nohup`, a service manager's
journal) each line is written once. With no password set, a non-loopback address's
warning goes to stderr the moment dux knows it, before anything else loads, whenever
stdout is not your terminal; on an interactive terminal the log line says it.

Two accepted oddities: `dux server | tee log` shows a warning twice on your terminal,
once through `tee` and once on stderr; and where stdout and stderr are two separate
pipes that something merges later (a container runtime, supervisord), that
no-password warning can land twice in the merged log.
After that it keeps a timestamped log going: browsers connecting and leaving, one line
per request (the access log), your Tailscale address coming and going, and the shutdown
when you stop it. Any warning about the start itself, a missing Tailscale for one, is
printed above the banner.
On a tailnet the banner also lists this machine's MagicDNS URL and, when
`tailscale serve` points at dux, its `https://` URL, and two QR codes follow it so a
phone can open dux from the screen (see
[QR codes for your phone](/docs/tailscale#qr-codes-for-your-phone)).
The port is 3890 because that is how you spell "dux" on a phone keypad.

The flags:

```text
dux server [OPTIONS]

  --bind <ADDR:PORT>   Bind this exact address, overriding [server] host+port.
                       An IP:port socket (hostnames are NOT resolved), e.g.
                       0.0.0.0:3890. May be given only once.
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
force-killing whatever is left. A second `Ctrl-c` stops waiting: dux kills whatever is
still running, logs what it killed, saves any settings change still queued, and exits,
which takes a moment at most. With nothing running, or before the wait has begun, a
second `Ctrl-c` exits at once, without waiting for a queued settings change to be
saved.

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
dux you already have starts serving in place. Your terminal turns into a themed dux
status screen showing the serve URLs (your MagicDNS and `tailscale serve` URLs included,
kept current while it runs), the same tailnet QR codes `dux server` prints, and a log
viewer. The viewer shows exactly the log
`dux server` prints, banner and access log included, so nothing you would see there is
missing here, the reachability note included: it is the banner's last row rather than a
line of its own in the header. The one deliberate exception is the warning about an
unrecognized `[server] color` value, which only `dux server` prints, because that
setting only colors `dux server`'s own output. The banner and the warnings above it are never dropped,
and they stay at the top of the panel for as long as they fit in half of it; on a
terminal too short for that they scroll with the rest instead of being cut off. Once the
buffer is full and you have scrolled back to its oldest line, each new line pushes that
oldest one out, so the view moves with it. Below them the viewer keeps the
last `log_viewer_lines` lines (2000 by default), and it scrolls by line, by page, and to
either end with the same scroll keys as the rest of dux; the in-app `?` help lists them
under the web server screen. While you are scrolled back, new lines do not move what you
are reading, and the bottom of the panel says how many arrived below. Press `q` or
`Esc` there to drop back into the TUI with everything still running, which stops serving
the web UI; you can flip again whenever you like. `Ctrl-c` quits dux entirely, winding
your agents down exactly as `dux server` does, and a second `Ctrl-c` while it waits for
them stops waiting and quits at once.

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
to config. When a run starts with this already on, the TUI's status line says so in the
warning color and holds the message longer than an ordinary note, so a listener that
came up before you sat down is not something you have to notice for yourself.

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

While it serves, the top bar grows a crumb right after the version: `● serving :3890` on
its own, becoming `● serving :3890 · 3 connected` once somebody is on it. The count is
browser tabs, not people, so one laptop with two tabs open counts as two, and a tab that
vanished without saying goodbye keeps counting until dux notices the socket is dead.

![The terminal UI top bar carrying a serving crumb with the port, above the usual workspace with one agent working.](/screens/tui-serving-crumb.png)

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
**Running in the background**, because the terminal kept going when its driver left;
press **Take over** once and it is yours. Either way the card carries
one button, **Take over**, and two ways to press it: click it, or, with the pane focused,
use the key that focuses an agent (Enter unless you have rebound it). Typing does not
claim a terminal, so keys pressed under the card go nowhere. An agent you start yourself
is yours straight away, with no card over it; agents dux reopens for you at startup wear
the **Running in the background** card until you press it. Switching the background
server on while dux is already running, with the palette command or by turning
`serve_while_tui` on in the config file, keeps everything you had running as yours: every
agent and terminal stays in your hands with no card over it, and a browser that wants one
presses **Take over**. When dux starts with the setting already on, it keeps nothing, so
a browser can pick up any of it. The card calls the terminal UI `the dux
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

## Who can get in

dux is a single-tenant, trusted-access tool: one owner, one workspace. It is safe to put on
a network with the right config, and the config is yours to get right. dux protects what
it can and says loudly when a setup is risky, but it serves the setup you chose.

> [!CAUTION]
> **Everyone who gets in shares one workspace.** They can attach to any agent or
> terminal, browse the server's filesystem through the project picker, run git actions,
> and see every session. Let in only people you would hand a terminal on that machine.

Three things decide who gets in:

- **Where dux listens.** Loopback by default: `127.0.0.1:3890` is reachable only from
  the machine dux runs on. Unless `tailscale = "no"`, dux also listens on your Tailscale
  address, so your tailnet devices reach it over WireGuard; on the default `"auto"` that
  listener comes and goes with the interface, with no restart (see
  [Reaching dux over Tailscale](/docs/tailscale)). `serve_while_tui = true` means a
  listener exists for as long as your terminal UI is open.
- **The password.** Optional, one password for one owner, set with
  `dux config set server.auth.password`. Once it is set, `[server.auth] require` decides
  who is asked for it; by default that is everyone except this machine and your own
  tailnet. See [The web password](/docs/web-login).
- **`blocked_addresses`.** Addresses and ranges dux refuses outright, with or without a
  password. dux adds to it after repeated failed sign-ins, and you can add to it too.

The safe shapes:

- **Loopback only**, the default. Nothing leaves the machine.
- **Your own tailnet.** Fine without a password when the tailnet is only you, though dux
  still raises its no-password warning (in the console, on the status line and as the
  browser banner) for as long as the Tailscale listener is up, because that listener
  reaches beyond this machine. When other people share the tailnet, set a password with
  `require = "tailnet"`.
- **A LAN or public address**, `--bind 0.0.0.0:3890` and friends, **with a password**.
  Over plain HTTP the password and the session can be read on the way, so prefer HTTPS:
  `tailscale serve`, or a reverse proxy of your own with `require = "everywhere"`.
  [Hosting dux on the public internet](/docs/public-hosting) is a worked example.

> [!WARNING]
> **No password on anything wider than loopback is on you.** dux still serves it, and
> warns as it starts (in red in `dux server`'s output and the flip's log viewer, in the
> warning color on the terminal UI's status line for the background server) and with a
> red banner in every browser, unless you turned that banner off with its **Don't show
> again**. Anyone who can reach that address controls your agents and worktrees.

> [!WARNING]
> **In a container, dux cannot see a Tailscale outside it.** A Tailscale on the host or in
> a sidecar container is invisible from inside dux's container, so when dux runs in one and
> sees no Tailscale, it serves and prints a warning once at start: it cannot tell whether
> something outside publishes its port, and a connection relayed onto that port looks like
> this machine. Keep the port private to your own network, or set a password with
> `require = "everywhere"`. See
> [When Tailscale isn't there](/docs/tailscale#when-tailscale-isnt-there).

Two automatic defenses always run as well. They stop a hostile web page from using your
own browser against your server, and they do nothing about a person who opens the URL:

- A **Host-header allowlist**, so a malicious page cannot DNS-rebind your browser into
  the server.
- A **same-origin check** on every socket upgrade and every write request, so another
  site cannot ride your session.

A Tailscale `100.x` IP is allowed automatically, whether or not that leg is bound at the
moment, and so is this machine's own MagicDNS name (`box.your-tailnet.ts.net`), on any
port, unless `tailscale = "no"`. dux reads that name from Tailscale and follows it if the
tailnet is renamed. Every other name, another machine's on the same tailnet included,
needs an `allowed_hosts` entry or the host guard answers `403`. See
[Your MagicDNS name just works](/docs/tailscale#your-magicdns-name-just-works) and
[HTTPS with `tailscale serve`](/docs/tailscale#https-with-tailscale-serve).

## The `[server]` config keys

Every key below carries a full inline comment in your `config.toml`:

```toml
[server]
# Bind host for `dux server`. An IP literal only (hostnames are not resolved):
# 127.0.0.1 is the loopback default, 0.0.0.0 is all interfaces. Serving from
# inside the TUI ignores this either way and always binds loopback (+ Tailscale).
host = "127.0.0.1"

# Bind port. Every way of serving uses it. The default is 3890, which is how
# you spell "dux" on a phone keypad.
port = 3890

# Whether dux also binds the machine's Tailscale address, so tailnet devices can
# reach it. "auto" (the default) binds it whenever the interface exists and keeps
# watching, so the listener comes and goes with your tailnet connection; "yes"
# binds it once and then stops looking; "no" never binds it. If the
# tailscale CLI is missing or the daemon is down, dux warns and keeps
# listening. Unless this is "no", dux also watches for a Tailscale Funnel or
# forward to its port, and while one is there, or while it cannot ask, it
# treats requests from this machine as the network ([server.auth] require).
# On "no" it cannot check at all, so it does that the whole time.
tailscale = "auto"

# Serve the web UI in the background while the terminal UI keeps running, on
# loopback plus the Tailscale address, exactly like the palette flip binds. Off
# by default. The start-background-server and stop-background-server palette
# commands flip it while dux runs and save the choice back here. With this on,
# a listener exists for as long as dux does; [server.auth] decides who is
# asked for a password.
serve_while_tui = false

# Extra Host header values to accept when a request is not same-origin. List a
# reverse-proxy hostname here so it is not rejected. This machine's own MagicDNS
# name needs no entry while tailscale is not "no". A config reload applies it.
allowed_hosts = []
```

The rest tune presentation and limits:

| Key | Default | What it does |
|---|---|---|
| `color` | `"auto"` | Colored, vite-style console output for `dux server` (`auto`, `always`, `never`). Read at startup. |
| `access_log` | `true` | Log a per-request line to the server's console: `dux server`'s output and the flip's log viewer alike (never to `dux.log`, so pipe `dux server`'s stdout to capture it). `/healthz` is always skipped. Set `false` to silence it in both. A config reload applies it. |
| `log_viewer_lines` | `2000` | How many lines the flip's log viewer keeps for scrolling back, between 1 and 20000: 0 is read as 1 and anything above 20000 as 20000, while a negative value is not valid there, so dux uses the default instead and says so in `dux.log`. The startup lines (the banner and its warnings) are kept on top of these and never dropped. `dux server` has no such cap, because its scrollback is your terminal's. Read when the flip starts. |
| `qr_codes` | `true` | Show QR codes for this machine's Tailscale IP and MagicDNS URLs (the `https://` one when `tailscale serve` points at dux) in `dux server` and on the start-web-server flip's status screen, side by side or stacked to fit, each with its URL under it. `dux server` prints them only when its output is a terminal; the background server never shows them. Read when a server starts. |
| `title` | `"dux"` | Web-only instance name: the browser tab title and the wordmark in the projects pane. Set `"dux (prod)"` to tell tabs apart. |
| `favicon` | `""` | Web-only favicon tint so several dux tabs are distinguishable. Empty keeps the yellow duck; otherwise a curated color (violet, blue, sky, cyan, teal, green, amber, orange, red, pink, rose). |
| `shutdown_timeout_seconds` | `30` | Seconds the server waits for agents and terminals to save state after SIGTERM before force-killing. A second Ctrl-c during the wait stops it, kills what is still running and exits, in `dux server` and in the flip alike. |
| `max_websocket_events_connections` | `32` | Cap on the status/event sockets (one per browser tab). |
| `max_websocket_agent_connections` | `32` | Cap on agent-PTY sockets. |
| `max_websocket_terminal_connections` | `64` | Cap on companion-terminal PTY sockets. |
| `max_websocket_tab_connections` | `64` | Cap on extra-tab PTY sockets across all agents (its own pool, so many-tab agents cannot starve others). |
| `max_websocket_tabs_per_agent` | `8` | Per-agent fairness sub-quota on that tab pool. |
| `file_drop_max_bytes` | `104857600` | Largest single file you can drag, or image you can paste, onto a terminal or agent pane in the browser (100 MiB). A bigger file is refused and nothing is written. `0` switches file drop and image paste off. |
| `file_drop_max_concurrency` | `2` | How many dropped-file uploads are accepted at once. Bounds buffered upload memory, not just queued work. An upload beyond the limit waits up to 30 seconds for a slot, then is refused with a `503` rather than queueing indefinitely. `0` clamps to `1`. |
| `search_index_max_files` | `50000` | Cap on the web editor's "Search files…" flat walk. `0` disables the cap. A config reload applies it. |
| `replay_wait_seconds` | `8` | How long a browser waits for the terminal's screen to arrive after connecting before it stops waiting quietly and offers a Reconnect button. Counted in time the page is actually on screen, so a phone in your pocket does not burn through it. `0` disables the wait, leaving a slow screen covered indefinitely. A config reload applies it. |
| `reconnect_backoff_cap_seconds` | `10` | The longest gap a browser leaves between automatic reconnect attempts. It starts at half a second and widens up to this. Raise it to be gentler on a struggling server, lower it to come back faster. A config reload applies it. |
| `reconnect_attempts` | `8` | How many times in a row a browser tries before it stops and says so, with a Reconnect button to start again. Any attempt that connects gives the whole budget back, and coming back to the tab, unlocking the phone or the network returning all start it over, so this only runs out on a page left sitting in front of a server that is not there. `0` keeps trying forever, though each attempt is still abandoned on the deadline below. A config reload applies it. |
| `reconnect_attempt_timeout_seconds` | `10` | How long one of those attempts may sit there without connecting before the browser abandons it and counts it as a failure. A server you cannot reach at all does not refuse the connection, it simply never answers, so without this an attempt could hang for most of a minute. It bounds a terminal pane's attach attempt too, not just the page's own connection. Too low and a slow connection never finishes connecting. A config reload applies it. |
| `changes_request_timeout_seconds` | `30` | How long the Changes pane waits for the list of changed files before it gives up and shows an error with a Refresh button. A connection that silently died never answers, and without this the pane would sit on "Loading changes" forever. The list for a worktree with tens of thousands of changed files can run to several megabytes, so too low and a slow connection never finishes loading it. `0` means the default, and anything above `600` is treated as `600`. A config reload applies it. |
| `heartbeat_seconds` | `15` | How often a visible browser tab checks its terminal connection is really alive. A Wi-Fi to cellular handoff can leave a connection that looks open and answers nothing, and this is what notices. A config reload applies it. |
| `heartbeat_deadline_seconds` | `30` | How long the browser waits for the answer to that check before deciding the connection is dead and reconnecting. Counted in time the page is on screen. Must be comfortably larger than `heartbeat_seconds`, or a slow network reconnects you needlessly; a value at or below it would reconnect over and over, so dux quietly uses twice `heartbeat_seconds` instead. A config reload applies it. |
| `pty_send_timeout_seconds` | `60` | How long dux waits for the first two things it sends a browser terminal, the handshake and the screen redraw, to actually arrive, before it gives up on that connection and lets the browser try again. A send finishes when the bytes get there, so on a slow connection this is really a measure of speed, and the screen redraw can be your whole scrollback. Set it too low and a phone on a bad signal can never finish attaching. A config reload applies it to the next terminal connection. |
| `tree_list_max_concurrency` | `8` | How many editor directory listings run at once. `0` disables the bound. Read at startup. |
| `release_notes_max_concurrency` | `2` | How many release-notes fetches run at once. `0` disables the bound. Read at startup. |
| `control_socket` | `"dux.sock"` | Where dux answers command-line clients on this machine: a socket file only your user can open, served by every running dux (the terminal app, the flip, background serving and `dux server` alike), with no password because your user account is the credential. A relative path is read from the config folder, an absolute one is used as written. A leftover socket from a dux that crashed is replaced at startup; anything at the path that is not a socket is left alone. A path longer than the system allows (103 bytes on macOS, 107 on Linux) starts dux without the socket, and dux says why on its status line and in `dux.log`. Read when dux starts; a config reload that changes it says the new path waits for the next start. |

> [!IMPORTANT]
> Most of these are read once, when serving starts, so changing them needs a **server
> restart**, not just a config reload: `host`, `port`,
> both `file_drop_*` keys, every connection cap, and the two `*_max_concurrency`
> limits. A config reload says so for all of them, on either surface: the browser
> and the terminal app each warn you.
>
> `color` is read once too, but only by `dux server`, which is the only way of
> serving that prints a console. A reload that changes it says so in the browser
> and tells you it applies the next time you start `dux server`; the terminal app
> stays quiet, because nothing it can start reads the setting. `qr_codes` is the same,
> read when `dux server` or the start-web-server flip starts serving.
>
> The exceptions are `allowed_hosts`, `access_log`, `search_index_max_files`,
> `pty_send_timeout_seconds` and the seven browser timing settings (`replay_wait_seconds`,
> `reconnect_backoff_cap_seconds`, `reconnect_attempts`,
> `reconnect_attempt_timeout_seconds`, `changes_request_timeout_seconds`,
> `heartbeat_seconds` and `heartbeat_deadline_seconds`), which a reload applies to a
> running server. Those seven describe what the BROWSER does, and an open tab picks them up on its own
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

The password and its neighbours have a section of their own, `[server.auth]`, listed with
every default in [Configuration](/docs/configuration#the-web-password-serverauth). Unlike
most of `[server]`, all of it applies on a reload.

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
  run **Reload config** from the same submenu afterward. If the file changed on disk since
  you opened it (a `dux config set`, a blocked address, another editor), the save is
  refused and nothing is written; you choose between reloading the file, which drops your
  edits, and keeping on editing so you can copy what you need first. An edit that sets,
  changes or removes the web password is refused whole; change the password in
  **Preferences…** instead, which asks for the current one. So is an edit that changes
  `[server] host` or `allowed_hosts`, including through an older setting name that dux
  turns into one of them: those stay a change you make in the file from a terminal.
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
  bound, how dux answers to your MagicDNS name, HTTPS with `tailscale serve`, the QR
  codes, and what plain HTTP costs you.
- [The web password](/docs/web-login): set a password, decide who is asked for it,
  and what it does and does not protect against.
- [Hosting dux on the public internet](/docs/public-hosting): a reverse proxy in front,
  with the dux password or `oauth2-proxy` and GitHub sign-in, in one Compose file.
