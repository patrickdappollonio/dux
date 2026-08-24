---
title: Attention indicators
description: How dux notices when an agent is waiting on you, lights up the sidebar, browser tab and favicon, and the per-agent settings that make each CLI speak up.
group: Guides
order: 35
---

An agent that is blocked on you looks a lot like an agent that is busy. Both sit there
with output on screen, and one of them has been waiting an hour for you to say "yes, run
that command." dux listens for the moment an agent asks, and says so where you are
looking.

## What you see

- **In the terminal UI**, a blinking accent-colored dot (`●`) takes over the agent's
  status dot in the sidebar, cyan in the default theme: two quick blinks, a steady hold,
  repeat. It wins over the working spinner, so an agent streaming its permission prompt
  reads as "needs you," not just "busy."
- **In the web UI**, the agent's icon turns cyan with the same rhythm, in the sidebar
  and in the mobile list, plus a small dot on the specific tab's pill when you are
  running several tabs. Everything holds still if you have reduced-motion turned on.
- **The browser tab title** gains a count in front of your configured instance name:
  `(2) dux` when two agents are waiting. A backgrounded tab updates the count without
  you visiting it.
- **The favicon** gets a small cyan dot in the corner of the duck while the count is
  above zero.

The flag clears the moment you look. Selecting the agent and focusing its terminal in
the terminal UI, opening its live view on the web, or typing into it all put the flag
down. An agent you are already watching never nags you.

> [!NOTE]
> When you step away entirely, dux stops treating the focused agent as watched, so a
> fresh request still lights up. Coming back holds the indicators for a few seconds
> instead of clearing them instantly, so you get a look at who wanted you. That grace
> window is `ui.attention_grace_seconds`, below, and it only applies right after you
> return: while you stay put, watching an agent still clears its flag at once.

## How dux detects it

dux watches the agent's terminal output for two things:

- **The terminal bell**, the classic ding. The most compatible signal.
- **Desktop-notification escape codes** (`OSC 9`, `OSC 99`, and `OSC 777`), the ones
  carrying a message like "Claude needs your permission."

> [!IMPORTANT]
> There is no formal "I need attention" protocol in the terminal world, so detection is
> best-effort. What an agent emits depends on the agent and on whether it recognizes the
> terminal it is running in. An agent that thinks it is in a bare, unknown terminal
> often emits nothing at all, which is why dux presents a real terminal identity; see
> [Terminal capabilities](/docs/terminal-capabilities).

## Turning it on per agent

Some agents need a one-line setting before they emit anything dux can see:

- **Claude Code**: with the default `terminal_identity = "auto"`, its automatic
  notification channel usually recognizes the terminal and just works. If yours shows
  nothing, set `preferredNotifChannel: "terminal_bell"` in its settings and dux catches
  the bell it rings on a permission prompt or a finished turn.
- **Codex**: set `tui.notification_method` in its config. Any value works. dux captures
  both the bell and the richer notification form.
- **Copilot**: it emits progress by default (its `terminalProgress` setting, on since
  v1.0.55), which dux reads with no setup. Its turn-completion bell went quiet by
  default in v1.0.60, so turn the terminal bell back on in Copilot's config if you want
  a finished turn flagged.
- **OpenCode**: no capturable signal out of the box today, because its notifications go
  through plugins. Its rows do not light up on their own. If a future version rings a
  bell or emits a notification, dux picks it up with no change on your side.

> [!NOTE]
> Any agent that continuously reports its busy or idle status, which Claude Code and
> Copilot both do, gets a truer "working" spinner for free, whether or not you turn
> notifications on.

## Settings

Two switches and a timer live in the `[ui]` section, all on by default:

```toml
[ui]
# Show an indicator when an agent asks for attention (a permission
# prompt, a finished turn). Detected from the agent's terminal
# notifications and bell. The TUI blinks a marker in the sidebar; the web
# UI shows a dot, a browser-tab count, and a favicon dot. Set to false to
# disable it everywhere.
attention_indicator = true

# Also treat a plain terminal bell as an attention request. The bell is
# the most compatible signal (Codex falls back to it; Claude Code emits it
# in terminal_bell mode) but can occasionally ring for mundane reasons, so
# turn this off if you find it noisy. Has no effect when
# attention_indicator is false.
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

Turn `attention_on_bell` off if a chatty tool inside the session, a test runner or tab
completion, rings the bell for reasons that are not about you. Turn
`attention_indicator` off to silence the whole feature.

> [!WARNING]
> The grace window needs a terminal that reports focus (DEC focus reporting, which
> kitty, ghostty, WezTerm, iTerm2, foot, alacritty and xterm all speak). Inside tmux,
> add `set -g focus-events on` to your `~/.tmux.conf`; note that tmux reports focus per
> pane, so switching tmux panes away from dux reads as unfocused. On a terminal that
> never reports focus the grace never applies and the focused agent's flag clears right
> away.

By default this feature stays inside dux: bells rung inside an agent's session are
consumed by dux and not passed on to the terminal you run dux in. Forwarding an agent's
real desktop notifications to your host terminal, or to your browser, is a separate
feature covered in [Terminal capabilities](/docs/terminal-capabilities). Only the
browser side needs an explicit permission grant.

## When nothing lights up

Work down this list before concluding it is broken:

1. **Check what terminal the agent thinks it is in.** This is the number one cause of
   silence. Open a companion terminal, which gets the same identity as the agent, and
   run `echo $TERM_PROGRAM`. Seeing your real terminal, or `ghostty` on the web, means
   identity is doing its job. An empty value is not proof of a problem: kitty,
   alacritty, foot and xterm never set it, and dux's forced `kitty` identity
   deliberately does not either. Check `[capabilities] terminal_identity` in your
   config, and under kitty look for `KITTY_WINDOW_ID` and `TERM` instead. Full story in
   [Terminal capabilities](/docs/terminal-capabilities).
2. **Give it a few seconds.** Some agents wait for a beat of true idleness before
   notifying; Claude Code holds off for about six seconds after a question appears. dux
   also holds fire briefly after you have just been typing at the agent, so a question
   you were clearly present for does not double-ding you.
3. **Remember that the agent you are watching never lights up.** To see the indicator
   fire, ask the agent something and then switch to a different agent or tab.
4. **Prove the plumbing with a forged signal.** Ask the agent to run
   `printf '\033]9;test\007'`. Those bytes land exactly like a real notification, so if
   the indicator lights up, dux works end to end and the silence is the agent's
   configuration. If even that does nothing, check that `attention_indicator` is still
   `true`.
5. **Know which silences are expected.** OpenCode has no capturable signal, and
   Copilot's turn-completion bell ships disabled.

> [!CAUTION]
> These signals are only bytes in the agent's output, so anything the agent displays
> that contains the same escape codes can forge or mask one. A printed notification code
> can flash a false "needs you," and a printed progress code can briefly claim "idle"
> when the opposite is true. This is inherent to terminal escape codes: a normal
> terminal pops the very same desktop notification for the very same bytes. A forged
> progress report only holds for a few seconds, and
> `attention_on_bell = false` or `attention_indicator = false` narrows or closes it.
