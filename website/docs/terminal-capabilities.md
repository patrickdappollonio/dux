---
title: Terminal capabilities
description: Present a real terminal identity to your agents so their notifications fire, forward those notifications and clipboard writes to your host terminal or browser, and make hyperlinks clickable.
group: Guides
order: 36
---

dux runs each agent inside an embedded terminal. On its own that means an agent cannot
tell what terminal it is really in, and anything it emits (a desktop notification, a
clipboard write, a clickable link) stops at dux. The `[capabilities]` section opens both
directions.

## Terminal identity

Agents decide whether to send desktop notifications by sniffing environment variables to
work out which terminal they are in. Hand one a bare, unrecognized terminal and several
agents, Claude Code among them, quietly send nothing. So dux tells the agent what
terminal it is really sitting in.

```toml
[capabilities]
terminal_identity = "auto"
```

The modes:

- **`auto`** (the default) does the right thing per surface. In the terminal UI it
  **mirrors your real terminal**, seeing through tmux. On the headless web server, where
  there is no host terminal to mirror, it presents **ghostty**, an identity the browser
  terminal renders well.
- **`mirror`** always mirrors the real host terminal, tmux see-through included.
- **`ghostty`**, **`iterm2`**, or **`kitty`** force that identity outright.
- **`none`** presents nothing. The agent inherits dux's own environment untouched, so
  agents like Claude Code will likely detect an unknown terminal and stay quiet. Reach
  for this if a forced identity ever confuses a tool.

> [!NOTE]
> `kitty` also sets `TERM=xterm-kitty`, which expects the kitty terminfo entry to be
> installed. If a program misrenders under that mode, that is why. `ghostty` and
> `iterm2` leave your `TERM` alone.

### Seeing through tmux

Run dux inside tmux and the terminal an agent would otherwise detect is tmux, not the
real thing. In `auto` and `mirror`, dux presents the outer terminal (kitty, ghostty, or
iTerm2) instead. It also strips the tmux markers, so the agent emits plain escape
sequences and dux re-wraps them when forwarding.

### Plain shells get an identity too

Identity is not an agent privilege. Every shell dux opens gets it, so a terminal you open
for yourself sees the same terminal an agent would. There are three kinds:

- A **companion terminal** belongs to an agent and opens in that agent's worktree.
- A **project terminal** belongs to a project and opens at its repo root.
- A **standalone terminal** belongs to nothing. It opens in your home directory, so you
  can have one before you have added a single project. Open one from the `⋯` menu beside
  the launcher button at the bottom of the sidebar, under **Terminals**, or from the cog
  menu's **New** submenu. Once you have one, the sidebar's Terminals divider grows a
  **+**.

> [!NOTE]
> Environment is where the standalone kind differs: the two owned kinds merge your global
> `[env]` with their project's, and a standalone terminal has no project, so it gets the
> global half and nothing else. None of the three runs a project's `startup_command` (see
> [Startup commands](/docs/startup-commands)).

Nothing closes a standalone terminal for you. Removing a project closes that project's
terminals and deleting an agent closes that agent's; neither touches one that belongs to
nobody. It ends when you close it, or when dux shuts down.

## Forwarding what the agent emits

A real identity means agents start emitting real notifications again. These switches
decide where those go.

```toml
[capabilities]
passthrough = true
clipboard_passthrough = "focused"
```

- **`passthrough`** is the master switch for sending an agent's notification, progress,
  and clipboard escape sequences **out of dux**. In the terminal UI that is the whole
  host forward: turn it off and your terminal receives nothing the agent emits. In the
  web UI the only thing forwarded outward is the clipboard write, so turning it off seals
  that.
- **`clipboard_passthrough`** governs clipboard writes on **both surfaces**:
  `"focused"` (only the agent tab you are looking at can write your clipboard, the safe
  default), `"always"` (any agent, background ones included), or `"off"` (never). It
  needs `passthrough = true`, and in the browser the write additionally only happens
  while the tab has focus.

> [!IMPORTANT]
> `passthrough` does **not** switch off browser desktop notifications. `web_notifications`
> below is the only setting for those. Clipboard **read** requests are never forwarded on
> either surface, because the reply would be typed straight back into dux.

In the terminal UI, notifications and progress reports forward from **every** agent,
background ones included, because a notification from an agent you are not watching is
precisely the one you want. In the web UI only the agent whose view is open can raise a
notification, and progress reports are not bridged at all.

> [!WARNING]
> Under tmux, forwarded notifications reach your terminal only if tmux is told to let
> them pass:
>
> ```tmux
> set -g allow-passthrough on
> ```
>
> Without it tmux swallows the forwarded sequences and the notifications simply never
> arrive.

## Browser notifications (web UI)

```toml
[capabilities]
web_notifications = true
```

In the web UI there is no host terminal to forward to, so dux bridges an agent's
notifications into real browser desktop notifications. This is the web-only switch for
them, and it has no effect on the terminal UI. Two things gate them, on purpose:

1. **The dux browser window has to be backgrounded.** dux never pops a desktop
   notification while you are looking at dux. The gate is the window being hidden or
   unfocused, not which agent you have selected.
2. **You have to opt in.** dux never auto-prompts for notification permission. Open
   **Preferences…** from the cog menu and use **Enable browser notifications** once. It
   sits under the **Desktop notifications** setting it unlocks, and appears only while
   notifications are enabled in config and you have not granted permission yet.

Clipboard writes work here too, under the same `clipboard_passthrough` switch: at
`"focused"` or `"always"` with the tab focused, an agent's write lands on **your**
browser's clipboard, not the server's.

## Clickable hyperlinks

```toml
[capabilities]
hyperlinks = true
```

Agents and plenty of CLI tools emit hyperlinks: text carrying a URL under the surface,
like a "View the PR" line that is secretly a link. With `hyperlinks` on, dux renders them
as real clickable links in the terminal UI (as long as your host terminal supports them)
and in the web terminal. The web opens only `http` and `https` links, in a fresh tab with
no way to reach back into the app.

In the terminal UI a click on a link opens it too, in the browser of the machine running
dux, and the agent never sees that click. It is the release that opens: press and let go
on the same link and it opens, and a sweep that ends anywhere else opens nothing. A sweep
that ends still on the link counts as a click on it, so to SELECT a URL under an agent
that is tracking the mouse, hold `Shift` and drag, which is the same force-a-selection
gesture that works anywhere else in the pane. Hold `Ctrl` to send that click to an agent
that is tracking the mouse; with no such agent there is nothing to send it to, so `Ctrl`
and a click just opens the link like any other. Your own terminal's link gesture still
works as it always did: dux holds the mouse, so in kitty that is `Shift` and a click, and
on dux's side `Shift` is the select-text modifier and never opens anything.

> [!IMPORTANT]
> In the web terminal, dux is the **only** thing that opens a link. While the agent has
> mouse reporting on, a click on a link is not passed through to it, because the agent
> would open the URL on the machine running dux rather than on the device in your hand.
> Every other click still reaches the app. To give the app a particular click, hold `Cmd`
> on macOS or `Ctrl` elsewhere; dux then opens nothing.

Selecting a link's text works the ordinary way: hold `Option` on macOS or `Shift`
elsewhere and drag, the same gesture that selects anywhere else while an app has the
mouse. That is a selection, not a click, so dux opens nothing.

## Pasting into the terminal UI

When you paste into the terminal UI, dux routes the text to whatever currently has your
keys: a text field (a modal, a filter, the commit box, where single-line fields fold line
breaks into spaces), or the agent when its pane is what you are typing into. If the
agent's CLI has bracketed paste on, which agent CLIs do, the CLI sees a single paste
rather than a burst of keystrokes. Otherwise the text arrives as plain typed input,
exactly as in any other terminal.

## Getting a file to the agent

Capabilities are about what an agent can reach out and do. The other direction, getting
something from your machine into the agent, is file drop and image paste in the browser:
drag a file onto the terminal, or paste a screenshot into it, and dux saves it on the
server and pastes its path. Web-only, because a real terminal emulator already types a
dropped file's path in for you. See
[Dropping and pasting files onto an agent](/docs/dropping-files).

## The fine print

> [!CAUTION]
> These are all bytes in the agent's terminal output, so a tool that prints one of these
> escape codes can forge a notification or a clipboard write. The same caveat from
> [Attention indicators](/docs/attention-indicators) applies. The blast radius is one
> notification or one clipboard write, and the clipboard default already limits writes to
> the agent you are focused on.

No single switch closes everything, because each governs a different way out. To seal
them all:

```toml
[capabilities]
terminal_identity = "none"       # inherit dux's own environment, present nothing
passthrough = false              # TUI: forward nothing to the host terminal
                                 # web: never mirror OSC 52 clipboard writes
clipboard_passthrough = "off"    # both surfaces: never mirror OSC 52 clipboard writes
hyperlinks = false               # both surfaces: render OSC 8 links as inert text,
                                 # with nothing left for a click to open
web_notifications = false        # web: no browser desktop notifications
```
