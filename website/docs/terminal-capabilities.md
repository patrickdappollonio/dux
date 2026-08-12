---
title: Terminal capabilities
description: Present a real terminal identity to your agents so their notifications actually fire, forward those notifications (and progress and clipboard writes) to your host terminal or browser, and make OSC 8 hyperlinks clickable.
group: Guides
order: 36
---

dux runs each agent inside an embedded terminal. That is great for isolation, but an
embedded terminal is also a natural capability black hole in both directions: on its
own, an agent cannot tell what terminal it is really in, and anything it emits (a
desktop notification, a clipboard write, a clickable link) would vanish into dux and
go no further. The `[capabilities]` section opens both directions.

## The identity problem, in one sentence

Agents decide whether to send desktop notifications by sniffing environment
variables to figure out which terminal they are in. If dux hands an agent a bare,
unrecognized terminal, several agents (Claude Code among them) quietly send
nothing. Under a kitty-plus-tmux setup a naive answer would leave the agent seeing
`TERM_PROGRAM=tmux`, at which point it gives up, which is exactly how notifications
come to look "broken" through no fault of your own.

So dux tells the agent, honestly, what terminal it is really sitting in.

## Terminal identity

```toml
[capabilities]
terminal_identity = "auto"
```

The modes:

- **`auto`** (the default) does the right thing per surface. In the terminal UI it
  **mirrors your real terminal**, even seeing through tmux (more on that below). On
  the headless web server, where there is no host terminal to mirror, it presents
  **ghostty**, an identity the browser terminal renders well.
- **`mirror`** always mirrors the real host terminal, including the tmux
  see-through.
- **`ghostty`**, **`iterm2`**, or **`kitty`** force that identity outright. Handy
  when you want the web UI (or the terminal UI) to look like a specific terminal to
  every agent. Note that `kitty` also sets `TERM=xterm-kitty`, which expects the
  kitty terminfo entry to be installed. If some program misrenders, that is why;
  `ghostty` and `iterm2` leave your `TERM` alone.
- **`none`** presents nothing at all. The agent inherits dux's own environment
  untouched, which means agents like Claude Code will likely detect an unknown
  terminal and stay quiet. Reach for this if a forced identity ever confuses a tool.

### Seeing through tmux

When you run dux inside tmux, the "terminal" the agent would otherwise detect is
tmux, not the real thing. In `auto` and `mirror`, dux peeks past tmux at the outer
terminal (kitty, ghostty, or iTerm2) and presents that instead. It also strips the
tmux markers from the agent so the agent emits plain, unwrapped escape sequences,
and dux re-wraps them itself when forwarding. You get your real terminal's identity
with tmux in the middle, which is the whole point.

### Plain shells get an identity too

The identity above is not an agent privilege. Every shell dux opens gets it, so a
terminal you open for yourself sees the same terminal an agent would, and the tools
you run in it behave the same way. There are three kinds and they differ only in
what they belong to:

- A **companion terminal** belongs to an agent and opens in that agent's worktree.
- A **project terminal** belongs to a project and opens at its repo root.
- A **standalone terminal** belongs to nothing at all. It opens in your home
  directory, which means you can have one before you have added a single project.
  Open one from the cog menu ("New standalone terminal").

Environment is the one place the third kind is genuinely different: the two owned
kinds merge your global `[env]` with their project's, and a standalone terminal has
no project, so it gets the global half and nothing else. None of the three runs a
project's `startup_command` (see
[Startup commands](/docs/startup-commands)), because that is worktree provisioning
for a new agent, not a shell rc.

Nothing closes a standalone terminal for you, either. Removing a project closes
that project's terminals and deleting an agent closes that agent's; neither event
has anything to do with one that belongs to nobody. It ends when you close it, or
when dux shuts down.

## Forwarding what the agent emits

Presenting a real identity means agents start emitting real notifications again.
The passthrough switches decide where those go.

```toml
[capabilities]
passthrough = true
clipboard_passthrough = "focused"
```

- **`passthrough`** is the master switch for sending an agent's notification,
  progress, and clipboard escape sequences **out of dux**. In the TUI that is the
  whole host forward: turn it off and your terminal receives nothing the agent
  emits. In the web UI the only thing an agent forwards outward is the `OSC 52`
  clipboard write, so turning it off seals that. It does **not** switch off
  browser desktop notifications; `web_notifications` below is the only setting
  for those.
- **`clipboard_passthrough`** governs the touchy one, `OSC 52` clipboard writes, on
  **both surfaces**: `"focused"` (only the agent tab you are actually looking at can
  write your clipboard, the safe default), `"always"` (any agent, even a background
  one), or `"off"` (never). It requires `passthrough = true` on both surfaces, and
  in the browser the write additionally only happens while the tab has focus.
  Clipboard **read** requests are never forwarded on either surface, because a
  reply would get typed straight back into dux.

In the terminal UI, notifications and progress reports forward from **every**
agent, background ones included, because a notification from an agent you are not
watching is precisely the one you want to see. In the web UI only the agent whose
view is currently open can raise a browser notification, because that is the only
PTY the browser is subscribed to, and progress reports are not bridged at all.

### One tmux gotcha

If you run dux under tmux and want forwarded notifications to actually reach your
terminal, tmux has to be told to let them pass:

```tmux
set -g allow-passthrough on
```

Without it, tmux swallows the forwarded sequences and you will wonder where your
notifications went. dux does the re-wrapping; tmux just has to agree to carry it.

## Browser notifications (web UI)

```toml
[capabilities]
web_notifications = true
```

In the web UI there is no host terminal to forward to, so dux bridges an agent's
notifications into real browser desktop notifications instead. `web_notifications`
is the **web-only** switch for this, and the only one: `passthrough` above does
not gate browser notifications, so sealing the clipboard leaves these working.
It has no effect on the TUI, whose host-terminal notifications are governed by
`passthrough` alone. Two things gate them, on purpose:

1. **The dux browser window has to be backgrounded.** dux never pops a desktop
   notification while you are looking at dux itself. The gate is the browser
   window being hidden or unfocused, not which agent you have selected.
2. **You have to opt in.** dux never auto-prompts for notification permission,
   because a surprise permission popup is nobody's idea of a good time. Open
   **Preferences…** from the cog menu and use **Enable browser notifications**
   once; your browser asks, you say yes, and you are set. It sits right under the
   **Desktop notifications** setting it unlocks, and only appears while
   notifications are enabled in config and you have not granted permission yet.

Clipboard writes work in the web UI too, governed by the same
`clipboard_passthrough` switch: when it is `"focused"` or `"always"` and the tab has
focus, an agent's `OSC 52` write is mirrored to **your** browser's clipboard, not
the server's. Set it to `"off"` to stop web clipboard writes entirely.

## Clickable hyperlinks

```toml
[capabilities]
hyperlinks = true
```

Agents (and plenty of CLI tools) emit `OSC 8` hyperlinks: text that carries a URL
under the surface, like a "View the PR" line that is secretly a link. With
`hyperlinks` on, dux renders them as real clickable links in the TUI (as long as
your host terminal supports OSC 8) and in the web terminal. For safety the web only
opens `http` and `https` links, and it opens them in a fresh tab with no way to
reach back into the app.

## Pasting into the terminal UI

Bracketed paste gets the same "honest middleman" treatment. When you paste into
the terminal UI, your host terminal hands dux the text as one paste event, and
dux routes it to whatever currently has your keys: a text field (a modal, a
filter, the commit box; single-line fields fold any line breaks into spaces), or
the agent itself when its pane is the thing you're typing into. If the agent's
CLI has turned bracketed paste on, which agent CLIs do, dux re-wraps the text in
the same paste markers so the CLI sees a single paste rather than a burst of
keystrokes. If it hasn't asked for bracketing, the text arrives as plain typed
input, exactly as it would in any other terminal.

## Getting a file to the agent

Capabilities are about what an agent can reach out and do. The other direction,
getting something from your machine INTO the agent, is covered by file drop and
image paste in the browser: drag a file onto the terminal, or paste a screenshot
into it, and dux saves it on the server and pastes its path. That is web-only,
because a real terminal emulator already types a dropped file's path in for you.
See
[Dropping and pasting files onto an agent](/docs/dropping-files).

## The fine print

These are all just bytes in the agent's terminal output, so the same honesty
caveat from [Attention indicators](/docs/attention-indicators) applies: a tool that
prints one of these escape codes can, in principle, forge a notification or a
clipboard write. The blast radius is small (one notification, one clipboard write),
every switch above can narrow or close it, and the clipboard default already limits
writes to the agent you are focused on. If you would rather keep everything sealed
inside dux, close each one; no single switch closes them all, because each governs
a different way out:

```toml
[capabilities]
terminal_identity = "none"       # inherit dux's own environment, present nothing
passthrough = false              # TUI: forward nothing to the host terminal
                                 # web: never mirror OSC 52 clipboard writes
clipboard_passthrough = "off"    # both surfaces: never mirror OSC 52 clipboard writes
hyperlinks = false               # both surfaces: render OSC 8 links as inert text
web_notifications = false        # web: no browser desktop notifications
```
