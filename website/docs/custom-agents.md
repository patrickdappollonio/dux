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

A tool can be a provider if and only if it supports two modes:

- **PTY mode:** an interactive session dux can embed in a pseudo-terminal. This is
  how you actually work with the agent.
- **Oneshot mode** (headless): hand it a prompt, get one response back. dux uses
  this for automated tasks like generating commit messages.

If your CLI can do both, dux can drive it.

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
# Oneshot args for non-interactive use (e.g. AI commit messages).
# Placeholders: {prompt} = the prompt text, {tempfile} = a temp file path.
oneshot_args = ["--bare", "-p", "{prompt}", "--tools", "", "--max-turns", "1"]
# Where to read the oneshot response from: "stdout" or "tempfile".
oneshot_output = "stdout"
# Hint shown to the user when the command isn't found on PATH.
install_hint = "curl -fsSL https://claude.ai/install.sh | bash"
# Where the mouse wheel and PgUp/PgDn go. Leave this key absent for auto: dux
# forwards them to the child only when it takes over the screen (a fullscreen,
# mouse-aware renderer like an agent's alt-screen UI) and otherwise scrolls its
# own host scrollback. Set true to always forward to the child, or false to
# never forward (always use dux scrollback).
# forward_scroll = true
```

### The oneshot placeholders

`oneshot_args` is a template. dux substitutes two placeholders before running it:

- `{prompt}`: the prompt text, inserted as a single argument.
- `{tempfile}`: the path to a temp file dux creates for the run.

How you read the result depends on the CLI:

- `oneshot_output = "stdout"`: dux captures the command's standard output. Use this
  when the CLI prints its answer.
- `oneshot_output = "tempfile"`: dux reads the file at `{tempfile}` after the
  command exits. Use this when the CLI writes its answer to a file you pass it.

## A worked example

Say you have a CLI called `myagent` that takes a prompt with `--prompt` for
interactive use and writes a oneshot answer to a file given by `--out`. The whole
integration is this:

```toml
[providers.myagent]
command = "myagent"
args = []
resume_args = []
oneshot_args = ["--out", "{tempfile}", "--prompt", "{prompt}"]
oneshot_output = "tempfile"
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
