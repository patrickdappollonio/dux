---
title: Custom CLI Agents
description: Configure any CLI as a dux provider, no adapters or protocol layer, just config.
group: Guides
order: 50
---

A provider is the CLI behind an agent. Claude Code, Codex, OpenCode, and Copilot are
configured out of the box, and **any other CLI can be a provider**. Adding one is a
config change, not a code change.

## The one rule

A tool can be a provider if and only if it supports **PTY mode**: an interactive
session dux can embed in a pseudo-terminal. If your CLI runs interactively in a
terminal, dux can drive it, with the same prompts, colors, and login flow it has in
your own shell.

## Anatomy of a provider

Providers live under `[providers.<name>]` in `config.toml`. The full set of fields:

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
# Where the mouse wheel and PgUp/PgDn go, in the windowed agent pane and in
# fullscreen alike. Leave this key absent for auto: dux forwards the wheel to
# the child when it asked for the mouse (a mouse-aware app like an agent's
# renderer) and the page keys when it owns the alt screen, and otherwise
# scrolls its own host scrollback. Set true to always forward, or false to never
# forward (always use dux scrollback).
# forward_scroll = true
# What a dragged, dropped or pasted file's path looks like when the web UI
# writes it into this provider's prompt. Web only, which is what the "web_"
# prefix says: in the terminal UI, dropping a file on the window is your
# terminal emulator's job. One of "bare", "single_quoted", "double_quoted" or
# "backslash_escaped"; absent means "bare".
web_dragdrop_paste = "bare"
```

`web_dragdrop_paste` takes `bare`, `single_quoted`, `double_quoted` or `backslash_escaped`.
An unrecognized value falls back to `bare`, and dux logs a warning.

> [!TIP]
> What each value writes into the prompt, and which one your CLI wants, is covered in
> [Dropping and pasting files onto an agent](/docs/dropping-files). In short: if a dropped
> path arrives as plain text instead of attaching, try `single_quoted`; if it arrives
> mangled with stray quotes or backslashes, the CLI wants it `bare`.

## A worked example

Say you have a CLI called `myagent` that you launch interactively with no extra
arguments and resume with `--continue`. The whole integration is this:

```toml
[providers.myagent]
command = "myagent"
args = []
resume_args = ["--continue"]
install_hint = "see https://example.com/install"
```

Save the config and apply it (run **Reload config** from the browser's cog menu or
`reload-config` in the terminal UI's palette, or restart dux), and `myagent` is a provider
you can use as a default or switch an agent or tab to. `forward_scroll` and
`web_dragdrop_paste` are left absent here, which means auto-detect scrolling and a bare
path.

## Choosing a provider per project

A project can pin a default provider so new agents start with the right CLI:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
default_provider = "myagent"
```

> [!NOTE]
> There is no provider picker in the create-agent prompt: a new agent uses its project's
> default provider.
>
> To move an existing agent to a different CLI, run the `change-agent-provider` palette
> command in the terminal UI, or pick **Change agent provider…** on the agent's `⋯` menu
> in the browser. It takes effect the next time that tab launches.
