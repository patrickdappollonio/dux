---
title: Terminal capabilities
description: Present a real terminal identity to your agents so their notifications actually fire, forward those notifications (and progress and clipboard writes) to your host terminal or browser, and make OSC 8 hyperlinks clickable.
group: Guides
order: 36
---

dux runs each agent inside an embedded terminal. That is great for isolation, but
it used to make dux a capability black hole in both directions: agents could not
tell what terminal they were really in, and anything they emitted (a desktop
notification, a clipboard write, a clickable link) vanished into dux and went no
further. The `[capabilities]` section fixes both directions.

## The identity problem, in one sentence

Agents decide whether to send desktop notifications by sniffing environment
variables to figure out which terminal they are in. If dux hands an agent a bare,
unrecognized terminal, several agents (Claude Code among them) quietly send
nothing. Under a kitty-plus-tmux setup the agent would see `TERM_PROGRAM=tmux` and
give up, which is exactly why notifications could look "broken" through no fault of
your own.

So dux now tells the agent, honestly, what terminal it is really sitting in.

## Terminal identity

```toml
[capabilities]
terminal_identity = "auto"
```

The modes:

- **`auto`** (the default) does the right thing per surface. In the TUI it
  **mirrors your real terminal**, even seeing through tmux (more on that below). On
  the headless web server, where there is no host terminal to mirror, it presents
  **ghostty**, an identity the browser terminal renders well.
- **`mirror`** always mirrors the real host terminal, including the tmux
  see-through.
- **`ghostty`**, **`iterm2`**, or **`kitty`** force that identity outright. Handy
  when you want the web UI (or a plain TUI) to look like a specific terminal to
  every agent. Note that `kitty` also sets `TERM=xterm-kitty`, which expects the
  kitty terminfo entry to be installed. If some program misrenders, that is why;
  `ghostty` and `iterm2` leave your `TERM` alone.
- **`none`** changes nothing. The agent inherits dux's environment exactly as it
  was before this feature existed. Reach for this if a forced identity ever
  confuses a tool.

### Seeing through tmux

When you run dux inside tmux, the "terminal" the agent would otherwise detect is
tmux, not the real thing. In `auto` and `mirror`, dux peeks past tmux at the outer
terminal (kitty, ghostty, or iTerm2) and presents that instead. It also strips the
tmux markers from the agent so the agent emits plain, unwrapped escape sequences,
and dux re-wraps them itself when forwarding. You get your real terminal's identity
with tmux in the middle, which is the whole point.

## Forwarding what the agent emits

Presenting a real identity means agents start emitting real notifications again.
The passthrough switches decide where those go.

```toml
[capabilities]
passthrough = true
clipboard_passthrough = "focused"
```

- **`passthrough`** is the **TUI-only** master switch for sending an agent's
  notification, progress, and clipboard escape sequences onward to your host
  terminal. Turn it off to keep everything the agent emits inside dux, the way it
  was before. It has no effect on the web UI, which uses `web_notifications` and
  `clipboard_passthrough` instead.
- **`clipboard_passthrough`** governs the touchy one, `OSC 52` clipboard writes, on
  **both surfaces**: `"focused"` (only the agent tab you are actually looking at can
  write your clipboard, the safe default), `"always"` (any agent, even a background
  one), or `"off"` (never). In the TUI it also requires `passthrough = true`; in the
  web UI it gates the browser clipboard write directly (and the browser itself only
  permits a write while the tab has focus). Clipboard **read** requests are never
  forwarded on either surface, because a reply would get typed straight back into
  dux.

Notifications and progress reports forward from **every** agent, background ones
included, because a notification from an agent you are not watching is precisely
the one you want to see.

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
is the **web-only** switch for this; it has no effect on the TUI, whose
host-terminal notifications are governed by `passthrough` above. Two things gate
them, on purpose:

1. **The tab has to be in the background.** dux never pops a desktop notification
   for an agent whose tab you are already staring at.
2. **You have to opt in.** dux never auto-prompts for notification permission,
   because a surprise permission popup is nobody's idea of a good time. Open the
   command palette (`Ctrl-K` / `Cmd-K`) and run **Enable browser notifications**
   once; your browser asks, you say yes, and you are set. The action only appears
   while notifications are enabled in config and you have not granted permission
   yet.

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

## The fine print

These are all just bytes in the agent's terminal output, so the same honesty
caveat from [Attention indicators](/docs/attention-indicators) applies: a tool that
prints one of these escape codes can, in principle, forge a notification or a
clipboard write. The blast radius is small (one notification, one clipboard write),
every switch above can narrow or close it, and the clipboard default already limits
writes to the agent you are focused on. If you would rather keep everything sealed
inside dux, the full "back where you started" set turns every switch off, each
scoped to its surface:

```toml
[capabilities]
terminal_identity = "none"       # inherit dux's own environment, present nothing
passthrough = false              # TUI: forward nothing to the host terminal
clipboard_passthrough = "off"    # both surfaces: never mirror OSC 52 clipboard writes
hyperlinks = false               # both surfaces: render OSC 8 links as inert text
web_notifications = false        # web: no browser desktop notifications
```
