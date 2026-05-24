---
title: FAQ
description: Quick answers to the small questions about platforms, providers, config, themes, and keybindings.
group: Reference
order: 100
---

Short questions, short answers. When a topic deserves more, the answer links to
the page that covers it in full.

## Installing & platforms

### Does dux run on Windows?

Through WSL2, which is Linux. dux targets macOS and Linux only; there is no
native Windows build, and WSL2 is the supported way to run it on Windows.

### What does dux cost?

Nothing. It's open source under the MIT license. You bring your own AI CLIs and
whatever accounts they need.

### Does dux phone home?

dux itself sends no telemetry. It launches local CLIs in local terminals: no
analytics, no JSON-RPC, no background uploads.

## Agents & providers

### Which AI tools can I use?

Claude Code, Codex, OpenCode, and Gemini are wired in out of the box, and any
other CLI that supports an interactive session and a headless one-shot mode can
be added. See [Custom CLI Agents](/docs/custom-agents).

### How do I add my own CLI as an agent?

Add a `[providers.<name>]` block to your config; no adapters, no protocol layer.
See [Custom CLI Agents](/docs/custom-agents).

### Can I start an agent from a GitHub PR?

Yes, when the `gh` CLI is installed and authenticated. See
[Creating an agent from a GitHub PR](/docs/creating-agents#creating-an-agent-from-a-github-pr).

### Do agents step on each other?

No. Each agent gets its own git worktree on its own branch, so two agents on the
same project run in complete isolation. See [Creating agents](/docs/creating-agents).

### Can I branch off a running agent?

Yes: fork it. dux makes a fresh worktree from the agent's current state,
uncommitted edits included. See
[Forking an existing agent](/docs/creating-agents#forking-an-existing-agent).

### Do I need the GitHub CLI?

It's optional. Install `gh` for PR tracking, creating agents from PRs, and
agent-opened PRs; skip it and dux quietly disables anything GitHub.

### Any recommended tools or MCP servers?

See [Recommended tools](/docs/recommended-tools) for providers, MCP servers,
skills, and companion CLIs that pair well with dux.

## Configuration

### Where does dux keep its config and data?

`~/.config/dux/` on Linux, `~/.dux/` on macOS. See
[where the config lives](/docs/configuration#where-it-lives).

### Is it safe to commit my config to git?

Yes. It stores portable intent, not secrets; env values stay as `${VAR}`
references. See
[environment variables and portable paths](/docs/configuration#environment-variables-and-portable-paths).

### How do I see what I've changed, or get the latest defaults?

`dux config diff` shows your changes; `dux config regenerate` previews the latest
template. See [managing the config](/docs/configuration#managing-the-config).

### How do I run setup before an agent starts?

Give the project a `startup_command`. See
[Startup commands & environment variables](/docs/startup-commands).

### What variables can my startup scripts read?

dux injects `DUX_WORKTREE_PATH` and friends into every startup command. See
[the injected variables](/docs/startup-commands#dux-injected-variables).

### What's a macro?

A reusable snippet of text you fire into an agent or a terminal with one
keystroke. See [Managing Macros](/docs/macros).

## Keybindings & themes

### How do I see every keyboard shortcut?

Press `?` in the app: the help overlay is the authoritative reference. Every
binding is configurable under `[keys]`. See
[keybindings](/docs/configuration#keybindings).

### How do I change the theme?

Set `theme` under `[ui]`, or open the theme picker for a live preview. See
[changing the theme](/docs/themes#changing-the-theme).

### How do I create my own theme?

Drop a TOML file in your themes directory and point your config at it. See
[writing your own theme](/docs/themes#writing-your-own-theme).
