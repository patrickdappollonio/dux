---
title: The workspace in the browser
description: The three-pane web layout, deep links, the browser terminals, the one-writer take-over model, copy-on-select and right-click paste, Shift-Enter soft newlines, companion terminals, and the mobile hub-and-spoke shell.
group: Server mode
order: 61
---

Open the server URL and you land in a workspace that mirrors the TUI, redrawn for
a browser. If you know the three-pane TUI, you already know your way around. If you
have never touched dux, this is a fine place to start, because the browser puts
everything one click away.

## The layout

On a desktop-width screen it is three panes, same as the TUI:

- A collapsible **left sidebar** lists your projects and the agents under each,
  grouped into projects that have agents and projects that do not. Drag rows to
  reorder projects and sessions. Toggle the sidebar with `Ctrl-B`.
- The **center pane** is the focused agent's live terminal, or a welcome screen
  when nothing is selected.
- The **right Changes pane** shows what the focused agent has changed. You can
  resize the split, or hide the Changes pane entirely when you want the terminal
  full width. See [Git without leaving the browser](/docs/web-git).

A slim header up top shows breadcrumbs (agent, provider, project, branch) and a
**Commands…** button that opens the command palette. There is no in-app `?` help
overlay in the browser the way there is in the TUI. Instead, the command palette
(`Ctrl-K` or `Cmd-K`) is your map: it lists the global commands, each with a short
description, and per-agent or per-file actions live in the `⋯` menu on the row
itself.

The web UI is dark-only today. There is no theme picker in the browser.

## Deep links

The URL tracks what you are looking at, so you can bookmark a session or paste a
link to a teammate on the same instance and they land exactly where you are:

- An agent is `#/agent/<sessionId>`.
- An extra provider tab is `#/agent/<sessionId>/tab/<tabId>`.
- A companion terminal is `#/agent/<sessionId>/terminal/<terminalId>`.

The links survive a reload and keep your browser back button working sensibly,
which matters most on the phone.

## The browser terminals

Each agent runs its real CLI in a real PTY on the server, and the browser streams
it live through a WebSocket. Closing the tab, losing connectivity, or your phone
falling asleep does not stop the agent: it keeps running on the server exactly as
it was until you explicitly kill or delete it (see
[Agents from the browser](/docs/web-agents)). The moment you open a terminal, dux
subscribes you to that PTY and, if the provider is not already running,
**launches or resumes it**. Opening the view is what starts the agent. The full
scrollback replays on connect, so you never open into a blank screen mid-session.

If the connection blips, dux shows a quiet "Reconnecting…" overlay and keeps your
buffer. Only after several failed attempts does it fall back to a blocking
"Connection lost" card with a Reconnect button.

### One writer, many watchers

Every device pointed at the same terminal sees the same output at the same time,
but **only one device at a time can type.** That device is the owner. Whoever
attaches with the tab in the foreground claims ownership automatically; attaching
with the tab in the background joins as a silent read-only watcher, so you can
peek without stealing the keyboard out from under whoever is driving.

A read-only view shows a full-cover card naming who has it ("Open on Chrome on
macOS") with a **Take over** button. Click it and input snaps to you, most-recent
claim wins. Nothing is lost, the other device simply becomes the watcher until it
takes over in turn. It is a polite hand-off, not a fight.

### Clipboard: the classic terminal model

The web terminal copies and pastes the way a real terminal does, no menu required:

- **Select to copy.** Highlight text and it lands on your clipboard immediately.
  A small "Copied to clipboard" toast confirms it. This is governed by the
  `ui.copy_on_select` preference (on by default), which you can flip from the
  command palette.
- **Right-click to paste** (with a mouse or pen). It reads your browser clipboard
  and sends it to the agent. On plain HTTP, where the browser blocks clipboard
  reads, dux nudges you toward `Ctrl+V` instead.
- A fixed set of chords works too, and it is not user-configurable: `Ctrl+Shift+C`,
  `Ctrl+Insert`, or `Cmd+C` to copy, and `Ctrl+V`, `Ctrl+Shift+V`, or `Cmd+V` to
  paste, with `Ctrl+C` staying SIGINT as it should.

There is deliberately no right-click context menu, because select-to-copy and
right-click-paste already cover both directions and a menu would only fight the
paste gesture.

One escape hatch worth knowing: when the app inside the terminal grabs the mouse
(think a full-screen TUI that does its own mouse handling), a plain drag goes to
that app instead of selecting text. Hold **Shift** (on Linux and Windows) or
**Option** (on macOS) while dragging to force a local selection you can copy. dux
pops a one-time hint the first time this bites you.

Your agent can also write your clipboard directly through an `OSC 52` escape
sequence, and that write lands on **your** browser's clipboard, not the server's,
governed by the `clipboard_passthrough` capability. That story lives in
[Terminal capabilities](/docs/terminal-capabilities).

### Shift-Enter for a soft newline

In the browser, **Shift-Enter inserts a newline instead of submitting.** Plain
Enter still submits. This is a web-only convenience (the TUI cannot tell the two
apart) that lets you compose a multi-line prompt without firing it off line by
line. It is careful never to fire mid-composition, so it will not mangle a CJK
input method's confirm keystroke.

## Companion terminals

An agent is not limited to its provider CLI. You can spawn **companion terminals**
alongside it, plain shells in the same worktree, for running tests, poking at git,
or tailing a log while the agent works. They nest under their agent in the sidebar
with their own icon, and the title tracks whatever is running in the foreground
("vim", "htop"). Unlike agents, which detach when you kill them, killing a
companion terminal destroys it.

## Macros

A floating **Macros…** button drops prewritten prompt snippets into the focused
terminal without submitting them, so you review and press Enter yourself. Each
macro is scoped to agents, terminals, or both, and multi-line macros are inserted
so they do not submit line by line. Full details in
[Managing Macros](/docs/macros).

## On your phone

Below tablet width, the web UI becomes a **hub-and-spoke** shell built for one
thumb, not a squished desktop:

- The **home** screen is the hub: your projects and sessions with the same `⋯`
  menus as desktop, an **Add project** button, and a **Search** button that opens
  the command palette. Tap a session to jump straight into its terminal.
- The **terminal** screen is a full-screen terminal with a slim bar on top (Back,
  branch name, an optional PR chip, and a chip showing the changed-file count) and
  the tab strip when the agent has more than one tab.
- The **changes** screen is the full Changes pane.

An **accessory bar** sits above the soft keyboard with the keys a phone keyboard
lacks: Esc, Tab, a sticky Ctrl and Alt latch, arrow keys, PgUp/PgDn, and a
dedicated **⇧↵** key for the soft newline (since a phone keyboard cannot produce
the Shift-Enter chord). Touch targets are sized generously so you are not
fat-fingering the wrong control. The soft keyboard is handled by the browser
shrinking the layout, so the accessory bar sits flush on top of the keyboard with
no fiddly per-pane math, and there is no fullscreen mode to fight with.

## Install it like an app

dux ships a small PWA manifest, so your browser will offer to add it to your home
screen or dock, where it opens standalone without browser chrome. The offline
story is deliberately minimal: the service worker caches only a small "dux is
unreachable" fallback page, nothing else. The app itself always loads fresh from
the server, so there is zero risk of a stale bundle talking to a newer server.
When you genuinely lose the connection mid-session, dux grays the app out behind a
"Reconnecting…" overlay and reconnects when it can.
