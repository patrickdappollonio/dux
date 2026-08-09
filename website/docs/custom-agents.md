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
# What a dragged, dropped or pasted file's path looks like when the web UI
# writes it into this provider's prompt. Web only, which is what the "web_" prefix says:
# in the terminal UI, dropping a file on the window is your terminal emulator's
# job. One of "bare", "single_quoted", "double_quoted" or "backslash_escaped";
# absent means "bare". See "Dropping and pasting files onto an agent" for which
# CLI needs which, and why.
web_dragdrop_paste = "bare"
```

A file at `/home/you/My Project/it's here.png` goes out as:

| Value | What is written into the prompt |
|---|---|
| `bare` | `/home/you/My Project/it's here.png` |
| `single_quoted` | `'/home/you/My Project/it'\''s here.png'` |
| `double_quoted` | `"/home/you/My Project/it's here.png"` |
| `backslash_escaped` | `/home/you/My\ Project/it\'s\ here.png` |

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
# web_dragdrop_paste left absent: "bare", the do-nothing form. If dropping a
# file on this agent in the browser leaves the path as plain text instead of
# attaching it, the CLI probably wants the path quoted; try "single_quoted". If
# the path arrives visibly mangled, with stray quote or backslash characters in
# it, the CLI wants it bare and you are already there.
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

New agents in this project use this provider. There is no provider picker in the
create-agent prompt: to move an existing agent to a different CLI, run the
`change-agent-provider` palette command in the terminal UI, or pick **Change agent
provider…** on the agent's `⋯` menu in the browser. It takes effect the next time
that tab launches.

## Why no adapters?

Because the CLI runs as-is. dux embeds a real terminal emulator and spawns the
command in a pseudo-terminal, so the tool behaves exactly like it does in your
shell: same prompts, same colors, same login flow, same everything. Keeping it
generic is what lets any future CLI become a provider with nothing more than a few
lines of TOML.
