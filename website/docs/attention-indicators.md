---
title: Attention indicators
description: How dux notices when an agent is waiting on you (a permission prompt, a finished turn) and lights up the sidebar, the browser tab, and the favicon, plus the one-line setting that turns it on per agent.
group: Guides
order: 35
---

An agent that is blocked on you looks a lot like an agent that is busy. Both sit
there with output on screen, and one of them has been waiting an hour for you to say
"yes, run that command." So dux listens for the moment an agent asks, and says so
where you are actually looking.

## What you see

When an agent pauses for you, the indicator lights up wherever you happen to be
looking:

- **In the terminal UI**, a blinking accent-colored dot (`●`) takes over the agent's status
  dot in the sidebar, cyan in the default theme: two quick blinks, a steady hold,
  repeat. It wins over the working spinner, so an agent that is streaming its
  permission prompt still reads as "needs you," not just "busy."
- **In the web UI**, the agent's icon itself turns cyan and does the same
  two-quick-blinks rhythm, in the sidebar and in the mobile list alike (plus a
  small dot on the specific tab's pill when you are running several tabs). If the
  agent was mid-bounce when it stopped to ask, the blink layers cleanly on top.
  Everything holds still if you have reduced-motion turned on.
- **The browser tab title** gains a count in front of your configured instance
  name: `(2) dux` when two agents are waiting. A backgrounded dux tab updates the
  count without you visiting it, so a glance at your tab strip is enough.
- **The favicon** gets a small cyan dot in the corner of the duck whenever the
  count is above zero, and goes back to the clean duck when everything is handled.

The flag clears the moment you look. Selecting the agent and focusing its terminal
(TUI) or opening its live view (web) puts the flag down, and so does typing into
it. An agent you are already watching never nags you.

There is a twist for when you step away entirely. While you are not looking at
dux at all, dux stops treating the focused agent as "being watched" so a fresh
attention request can still light up: in the web UI that means your dux browser
tab is hidden, and in the TUI it means your terminal window (or tmux pane) has
lost focus. Then, the moment you come back, dux keeps the indicators up for a
few seconds instead of dismissing them the instant focus returns, so you
actually get a look at which agent(s) wanted you before the flag clears. That
grace window is configurable (`ui.attention_grace_seconds`, covered below) and
only applies right after you return; while you stay put, watching an agent still
clears its flag immediately as usual.

## How dux detects it

dux runs each agent inside its own embedded terminal, so it sees exactly what the
agent tells that terminal. It watches for two things:

- **The terminal bell** (the classic ding). The most compatible signal.
- **Desktop-notification escape codes** (`OSC 9`, `OSC 99`, and `OSC 777`), the
  ones that carry a message like "Claude needs your permission." `OSC 99` is the
  kitty notification protocol, which some agents prefer when they detect a
  kitty-family terminal.

There is no formal "I need attention" protocol in the terminal world, so detection
is best-effort by nature. What an agent emits depends on the agent, and on whether
it recognizes the terminal it is running in. That last point is why dux presents a
real terminal identity to the agent (see
[Terminal capabilities](/docs/terminal-capabilities)): an agent that thinks it is
running in a bare, unknown terminal often emits nothing at all.

## Turning it on per agent

Some agents need a one-line setting before they emit anything dux can see:

- **Claude Code**: with dux presenting a real terminal identity (the default
  `terminal_identity = "auto"`, see
  [Terminal capabilities](/docs/terminal-capabilities)), Claude Code's automatic
  notification channel usually recognizes the terminal and just works, no config
  needed. If your setup still shows nothing (an older Claude, an unusual terminal),
  fall back to setting `preferredNotifChannel: "terminal_bell"` in its settings and
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

# Seconds the attention indicators stay visible after dux regains your
# attention, before the focused agent's needs-attention flag clears. Applies
# when you return to the dux browser tab (web UI) and when your terminal
# window regains focus (TUI). Gives you time to see which agent(s) wanted you
# before the indicator vanishes. Set to 0 to clear the indicator immediately.
# TUI note: requires a terminal that reports focus; under tmux, set
# `focus-events on`. Without focus reports the grace never applies: the
# focused agent's indicator clears right away.
attention_grace_seconds = 3
```

Turn `attention_on_bell` off if a chatty tool inside your agent's session (a test
runner, tab completion) rings the bell for reasons that are not really about you.
Turn `attention_indicator` off to silence the whole feature everywhere.
`attention_grace_seconds` covers both surfaces. In the terminal UI it rides on your
terminal telling dux when its window gains or loses focus (DEC focus reporting,
which kitty, ghostty, WezTerm, iTerm2, foot, alacritty, and xterm all speak): dux
stops clearing the focused agent's flag while your window is unfocused, then holds
the indicators for this many seconds once you switch back. Running inside tmux? Add
`set -g focus-events on` to your `~/.tmux.conf` so tmux forwards those focus events
(note tmux reports focus per pane, so switching tmux panes away from dux reads as
unfocused too). On a terminal that never reports focus the grace window simply does
not apply: dux assumes you are always looking, and a focused agent's flag clears
immediately as usual.

By default this feature stays inside dux: bells rung inside an agent's session are
consumed by dux's embedded terminal and not re-forwarded to the terminal you run
dux in. If you would rather have the agent's real desktop notifications reach your
host terminal (or your browser, in the web UI), that is a separate feature, on by
default in config and covered in
[Terminal capabilities](/docs/terminal-capabilities). Only the browser side needs
an explicit permission grant.

## When nothing lights up

Detection depends on the agent choosing to speak, and agents only speak to
terminals they recognize. Work down this list before concluding it is broken:

1. **Check what terminal the agent thinks it is in.** This is the number one
   cause of silence. In the TUI, dux mirrors your real terminal's identity, even
   seeing through tmux to the terminal underneath it; in the web UI it presents
   ghostty, an identity the browser terminal renders well. Open a companion
   terminal (which gets the same identity as the agent) and run
   `echo $TERM_PROGRAM`. If you see your real terminal (or `ghostty` on the web),
   identity is doing its job. An empty `TERM_PROGRAM` is not proof of a problem on
   its own: several terminals (kitty, alacritty, foot, xterm) never set it, and
   dux's forced `kitty` identity deliberately does not either. Check
   `[capabilities] terminal_identity` in your config, and under kitty look for its
   own markers (`KITTY_WINDOW_ID`, `TERM`) instead. The full story lives in
   [Terminal capabilities](/docs/terminal-capabilities).
2. **Give it a few seconds.** Some agents wait for a beat of true idleness before
   notifying (Claude Code holds off for about six seconds after a question
   appears), and dux itself holds fire briefly after you have just been typing at
   the agent, so a question you were clearly present for does not double-ding
   you. A short pause between "the agent stopped" and "the indicator lit" is
   normal, not a miss.
3. **Remember that the agent you are watching never lights up.** By design.
   Focused terminal in the TUI, open live view in the web: both count as "you are
   already looking," and the flag stays down. To see the indicator fire, ask the
   agent something and then switch to a different agent or tab.
4. **Prove the plumbing with a forged signal.** Ask the agent itself to run
   `printf '\033]9;test\007'`. Those bytes land in the agent's terminal exactly
   like a real notification would, so if the indicator lights up, dux's side
   works end to end and the silence is the agent's configuration (see the
   per-agent list above). If even that does nothing, check that
   `attention_indicator` is still `true`.
5. **Know which silences are expected.** OpenCode has no capturable signal out of
   the box, and Copilot's turn-completion bell ships disabled, both covered in
   the per-agent list above.

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
