---
title: Custom CLI Agents
description: Configure any CLI as a dux provider, no adapters or protocol layer, just config.
group: Guides
order: 50
---

A provider is the CLI behind an agent. Claude Code, Codex, OpenCode, and Copilot are
configured out of the box, but the whole point of dux's design is that **any CLI can
be a provider.** Adding one is a config change, not a code change. There are no
adapters and no protocol layer to implement. dux runs the command exactly as it
would in a normal terminal.

## The one rule

A tool can be a provider if and only if it supports **PTY mode**: an interactive
session dux can embed in a pseudo-terminal. That's how you actually work with the
agent. If your CLI can run interactively in a terminal, dux can drive it.

## Anatomy of a provider

Providers live under `[providers.<name>]` in `config.toml`. Here's the full set of
fields, with what each one does:

```toml
[providers.claude]
# The CLI command for this provider's sessions.
command = "claude"
# Arguments passed when launching an interactive PTY session.
args = []
# Optional args used when reconnecting a detached session. Leave empty for CLIs
# that don't support resuming a session scoped to the working directory.
resume_args = ["--continue"]
# Optional timeout (ms) for a resumed session that renders nothing. If a resume
# hangs before showing output, dux kills it and starts fresh. 0 disables it.
resume_wait_timeout_ms = 0
# Hint shown to the user when the command isn't found on PATH.
install_hint = "curl -fsSL https://claude.ai/install.sh | bash"
# Where the mouse wheel and PgUp/PgDn go. Leave this key absent for auto: dux
# forwards them to the child only when it takes over the screen (a fullscreen,
# mouse-aware renderer like an agent's alt-screen UI) and otherwise scrolls its
# own host scrollback. Set true to always forward to the child, or false to
# never forward (always use dux scrollback).
# forward_scroll = true
```

## A worked example

Say you have a CLI called `myagent` that you launch interactively with no extra
arguments and resume with `--continue`. The whole integration is this:

```toml
[providers.myagent]
command = "myagent"
args = []
resume_args = ["--continue"]
install_hint = "see https://example.com/install"
# forward_scroll left absent: auto-detect (forward only to a fullscreen,
# mouse-aware child, otherwise dux host scrollback).
```

Save the config, and `myagent` is now a provider you can pick when creating an
agent. That's the entire process.

## Choosing a provider per project

A project can pin a default provider so new agents start with the right CLI:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
default_provider = "myagent"
```

You can still pick a different provider when you create an individual agent; this
just sets the default.

## Why no adapters?

Because the CLI runs as-is. dux embeds a real terminal emulator and spawns the
command in a pseudo-terminal, so the tool behaves exactly like it does in your
shell: same prompts, same colors, same login flow, same everything. Keeping it
generic is what lets any future CLI become a provider with nothing more than a few
lines of TOML.
