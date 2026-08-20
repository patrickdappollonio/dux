---
title: FAQ
description: Quick answers to the small questions about platforms, providers, server mode and the browser, config, themes, and keybindings.
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

Claude Code, Codex, OpenCode, and Copilot are wired in out of the box, and any
other CLI that runs an interactive session in a terminal can be added. See
[Custom CLI Agents](/docs/custom-agents).

### Why was the Gemini provider removed?

[Google deprecated the Gemini CLI](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/),
so dux no longer ships it as a built-in provider. If a worktree was still pinned
to Gemini, dux won't launch it — switch it to a supported provider and relaunch.

### How do I add my own CLI as an agent?

Add a `[providers.<name>]` block to your config; no adapters, no protocol layer.
See [Custom CLI Agents](/docs/custom-agents).

### Do I have to go fullscreen to type to an agent?

No. In the terminal UI, focusing the agent's pane is enough: what you type goes
to the agent right there in the windowed layout, while dux's own shortcuts (all
modifier chords) keep working. Fullscreen is still there as a toggle for when
the agent should get every key verbatim; the keys dux otherwise keeps for
itself reach the agent there, so it is the escape hatch for Tab completion and
readline shortcuts. The in-app help overlay shows the toggle's current binding.

### The mouse wheel or PgUp won't scroll an agent. Why?

Some agents take over the whole screen and scroll their own content. A good
recent example is Claude Code's new full-screen renderer (OpenCode works the
same way): dux detects this and forwards the wheel and `PgUp`/`PgDn` to the
agent, while keeping its own scrollback for agents that don't. The same rule
applies whether the agent pane is windowed or fullscreen. An explicit
`forward_scroll = true`/`false` in a `[providers.<name>]` block overrides that
detection; delete the line to return to auto-detect. See
[Custom CLI Agents](/docs/custom-agents).

The mobile web client behaves the same by touch: over a full-screen agent a
one-finger drag and the on-screen `PgUp`/`PgDn` buttons forward to the agent
instead of moving an empty scrollback. The web auto-detects from the agent's
mouse mode (the `forward_scroll` config lever applies to the terminal UI only).

### Can I start an agent from a GitHub PR?

Yes, when the `gh` CLI is installed and authenticated. See
[Creating an agent from a GitHub PR](/docs/creating-agents#creating-an-agent-from-a-github-pr).

### Do agents step on each other?

No. Each agent in a project gets its own git worktree on its own branch, so two
agents on the same project run in complete isolation. A standalone agent runs in a
folder you picked, and dux refuses to put a second agent in a directory one is
already working in, for the same reason: coding CLIs resume their conversation per
directory. See [Creating agents](/docs/creating-agents).

### Can I branch off a running agent?

Yes: fork it. dux makes a fresh worktree from the agent's current state,
uncommitted edits included. See
[Forking an existing agent](/docs/creating-agents#forking-an-existing-agent).

### Do I need the GitHub CLI?

It's optional. Install `gh` for PR tracking, creating agents from PRs, and
agent-opened PRs; skip it and dux quietly disables anything GitHub.

### Any recommended tools or MCP servers?

See [Recommended tools](/docs/recommended-tools) for providers, MCP servers,
and skills that pair well with dux.

## Server mode & the browser

### Do I have to keep a terminal open to use dux in a browser?

No. `dux server` runs the web UI on its own, with no terminal UI in front of it, so
it is happy under `systemd`, `tmux`, or anything else that keeps a process alive. See
[Server mode overview](/docs/server-mode).

### Is there a login?

No. There is no password, no token, and no user accounts, on purpose: dux is a
single-tenant, trusted-access tool. Anyone who can reach the port gets the whole
workspace, including typing into your agents. That is why it binds `127.0.0.1` by
default. See [the trust model](/docs/server-mode#the-trust-model-stated-plainly).

### Is server mode a hosted service? Does my code leave my machine?

Neither. There is no dux cloud and no account to make. `dux server` is the same
binary serving a web UI from your own machine, over your own network, and your repos
never leave it. Your agents' own CLIs talk to whatever AI providers they always talk
to; dux adds no traffic of its own.

### Can I run the terminal UI and the browser at the same time?

Not simultaneously, and you do not need to. One dux process owns a config directory
at a time (they share a single-instance lock), so a second one fails fast rather than
two processes fighting over the same database. Instead you hand the workspace across:
the `start-web-server` palette command flips a running terminal UI into serving the
browser, and pressing `q` there hands it back. Your agents keep running through every
transition. See [Two ways to start it](/docs/server-mode#two-ways-to-start-it).

### Can I reach it from my phone?

Yes. dux binds your Tailscale address by default, so any device on your tailnet can
open it. Read [Reaching dux over Tailscale](/docs/tailscale) first, because there is
no login in front of it.

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
template. The summary holds back `[env]` values and project details, so it is
safe to share, but `dux config diff --raw` prints your whole config including
those values. See
[what `dux config diff` shows](/docs/configuration#what-dux-config-diff-shows-and-what-it-holds-back).

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

Open the help overlay in the terminal UI; its key is shown in the footer hint bar,
and it is the authoritative reference. Every
binding is configurable under `[keys]`. See
[keybindings](/docs/configuration#keybindings).

### How do I change the theme?

Set `theme` under `[ui]`, or open the theme picker for a live preview. See
[changing the theme](/docs/themes#changing-the-theme).

### How do I create my own theme?

Drop a TOML file in your themes directory and point your config at it. See
[writing your own theme](/docs/themes#writing-your-own-theme).
