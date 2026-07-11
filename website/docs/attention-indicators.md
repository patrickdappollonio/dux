---
title: Attention indicators
description: How dux notices when an agent is waiting on you (a permission prompt, a finished turn) and lights up the sidebar, the browser tab, and the favicon, plus the one-line setting that turns it on per agent.
group: Guides
order: 35
---

An agent can sit blocked for an hour, patiently waiting for you to say "yes, run
that command" while its sidebar row looks exactly like a happily-working one. dux
fixes that. When an agent needs you, it says so, and now dux listens.

## What you see

When an agent pauses for you, the indicator lights up wherever you happen to be
looking:

- **In the TUI**, a blinking amber diamond (`◆`) takes over the agent's status dot
  in the sidebar. It wins over the working spinner, so an agent that is streaming
  its permission prompt still reads as "needs you," not just "busy."
- **In the web UI**, an amber dot appears next to the agent's name in the sidebar
  (and on the specific tab's pill when you are running several tabs). The dot
  pulses gently, and holds still if you have reduced-motion turned on.
- **The browser tab title** gains a count in front of your configured instance
  name: `(2) dux` when two agents are waiting. A backgrounded dux tab updates the
  count without you visiting it, so a glance at your tab strip is enough.
- **The favicon** gets a small amber dot in the corner of the duck whenever the
  count is above zero, and goes back to the clean duck when everything is handled.

The flag clears the moment you look. Selecting the agent and focusing its terminal
(TUI) or opening its live view (web) puts the flag down, and so does typing into
it. An agent you are already watching never nags you.

## How dux detects it

dux runs each agent inside its own embedded terminal, so it sees exactly what the
agent tells that terminal. It watches for two things:

- **The terminal bell** (the classic ding). The most compatible signal.
- **Desktop-notification escape codes** (`OSC 9` and `OSC 777`), the ones that
  carry a message like "Claude needs your permission."

There is no formal "I need attention" protocol in the terminal world, so detection
is best-effort by nature. What an agent emits depends on the agent, and on whether
it recognizes the terminal it is running in.

## Turning it on per agent

Some agents need a one-line setting before they emit anything dux can see:

- **Claude Code**: set `preferredNotifChannel: "terminal_bell"` in its settings.
  dux catches the bell it then rings on a permission prompt or a finished turn.
- **Codex**: set `tui.notification_method` in its config (any value works). dux
  captures both the bell and the richer notification form.
- **Copilot**: it already ships a truer "working" spinner for free. Copilot CLI
  emits OSC 9;4 progress by default (its `terminalProgress` setting, on since
  v1.0.55), which dux reads with zero setup. Its turn-completion bell, on the
  other hand, went quiet by default in v1.0.60 ("terminal bell no longer sounds on
  turn completion unless explicitly enabled via config"), so flip the terminal
  bell back on in Copilot's config if you want dux to flag a finished turn.
- **OpenCode**: no capturable signal out of the box today (its notifications go
  through plugins), so its rows will not light up on their own. This is expected,
  not a bug. If a future version starts ringing a bell or emitting a notification,
  dux picks it up with no change on your side.

Because a bonus of the same detector: any agent that continuously reports its
busy/idle status (Claude Code and Copilot both do, through a progress escape code)
gets a truer "working" spinner for free, whether or not you turn on
notifications.

## Settings

Two switches live in the `[ui]` section of your config, both on by default:

```toml
[ui]
# Show an indicator when an agent asks for attention (permission prompts,
# finished turns). Detected from the agent's terminal notifications.
attention_indicator = true

# Also treat a plain terminal bell as an attention request. The bell is the
# most compatible signal (Codex falls back to it; Claude Code emits it in
# terminal_bell mode) but can occasionally ring for mundane reasons.
attention_on_bell = true
```

Turn `attention_on_bell` off if a chatty tool inside your agent's session (a test
runner, tab completion) rings the bell for reasons that are not really about you.
Turn `attention_indicator` off to silence the whole feature everywhere.

Nothing about this makes dux noisier in your real terminal: bells rung inside an
agent's session are consumed by dux's embedded terminal and never re-forwarded to
the terminal you are running dux in.

## Limitations

These signals are just bytes in the agent's terminal output, so anything the
agent chooses to display (a file it prints, a tool's output) that happens to
contain the same escape codes can forge or mask a signal: a printed notification
code can flash a false "needs you," and a printed progress code can briefly say
"idle" or "working" when the opposite is true. This is inherent to terminal escape
codes, not specific to dux. A normal terminal pops the very same desktop
notification for the very same bytes. The blast radius is small and bounded: a
forged progress report only holds sway for a few seconds before dux falls back to
watching real output, and the whole feature is behind the two switches above, so
`attention_on_bell = false` or `attention_indicator = false` narrows or closes it
entirely.
