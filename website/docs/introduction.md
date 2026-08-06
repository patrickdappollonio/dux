---
title: Introduction
description: What dux is, the mental model behind it, and how the pieces fit together.
group: Getting started
order: 1
---

`dux` runs multiple AI coding agents in parallel, one git worktree each. It spawns
the real CLI for each agent (Claude Code, Codex, Copilot, OpenCode, or anything
else you can run in a terminal) inside an embedded pseudo-terminal. No protocol
layer, no adapters, no JSON-RPC. Just the tools you already use, side by side,
each in its own branch.

## Two front ends, one engine

dux has two front ends over one engine: a terminal UI and a web UI. Both are first
class, and both are staying. They share the same projects, the same agents, the
same worktrees and the same config file, so an agent you start in one is the same
agent in the other.

They are not identical, on purpose. Each surface does what its medium is good at.
The terminal gives you full keyboard control, rebindable keys, a command palette
and themes. The browser gives you reach: any device on your network, including a
phone, plus editing files in the page and desktop notifications. Where a
capability only makes sense on one side, it lives on one side, and the page that
covers it says why.

To know whether something is available where you are, the surface itself is the
answer: the terminal's help overlay and command palette list what it can do, and
the browser's cog menu and row menus list what it can do.

The web UI is [server mode](/docs/server-mode), started with `dux server` or flipped
on from a running terminal UI. You run one front end at a time (one dux process owns
your config directory), and moving between them is a hand-off that leaves your agents
running. Everything else on this page is true of both.

## The mental model

dux has three nouns. Once they click, the whole app makes sense.

- **Projects** are git repositories you've added to dux. A project points at a
  checkout on disk and remembers your preferences for it.
- **Agents** are sessions running inside a project. Each agent gets its own git
  worktree on its own branch, so two agents working on the same repo never step on
  each other.
- **Providers** are the CLIs that power agents. Claude, Codex, OpenCode, and Copilot
  ship configured out of the box, and you can wire up any other command yourself.

The flow is: add a project, spin up an agent, pick a provider. dux creates the
worktree, launches the CLI in a real terminal, and tracks the session so you can
walk away and reconnect later.

## The three panes

Both front ends lay the workspace out the same way, in three panes:

- The **left pane** lists your agents in a single flat list, most-active first,
  with a search filter and a project chooser for creating or targeting a project.
- The **center pane** shows the focused agent's live terminal output. In the
  terminal UI it doubles as the diff view when you review changes; in the browser
  a diff opens in the file-editor overlay instead.
- The **right pane** shows the files an agent has changed, with diffs.

In the terminal UI, dedicated focus-next and focus-previous keys move between
panes, and that's the primary way you get around. Every pane has its own local key
combinations, and the authoritative list of every binding lives in the in-app help
overlay.
Everything is rebindable; see [Configuration](/docs/configuration) for how. In the
browser the same three panes are click-driven, with a collapsible sidebar and a
resizable Changes split; see
[The workspace in the browser](/docs/web-workspace).

## Where dux keeps its files

dux stores everything in one directory:

- **Linux:** `~/.config/dux/`
- **macOS:** `~/.dux/`

Inside you'll find `config.toml` (your settings), `sessions.sqlite3` (session
state), `dux.log` (logs, the first place to look when something misbehaves), and a
`themes/` directory for any themes you write yourself.

That directory is **yours alone**: dux makes it `0700` on every startup, and the
files it writes inside `0600`. Both `config.toml` and the session database can
hold environment variables you set for a project, which is exactly where an API
token tends to live, so no other user on the machine gets to read them. If you
are upgrading and your directory is currently world-readable, the next start
tightens it for you. dux only ever removes group and world access, never your
own, so a config file you have deliberately made read-only stays that way.

## Where to go next

- [Server mode overview](/docs/server-mode): the web UI, how to start it, and the
  no-login trust model to understand before you expose it.
- [The workspace in the browser](/docs/web-workspace): the browser layout, its
  terminals, and the phone experience.
- [Reaching dux over Tailscale](/docs/tailscale): open your workspace on your phone,
  and the caveats that come with it.
- [Configuration](/docs/configuration): the config file, where it lives, and how
  it expands environment variables.
- [First run & what's new](/docs/first-run-and-whats-new): the welcome screen a
  fresh install gets, the what's-new screen after an update, and how to turn either
  automatic screen off.
- [Managing Themes](/docs/themes): switch the look, or build your own.
- [Custom CLI Agents](/docs/custom-agents): teach dux to drive any CLI you like.
