---
title: Introduction
description: What dux is, the mental model behind it, and how the pieces fit together.
group: Getting started
order: 1
---

`dux` is a terminal UI for running multiple AI coding agents in parallel, one git
worktree each. It spawns the real CLI for each agent — Claude Code, Codex, Gemini,
OpenCode, or anything else you can run in a terminal — inside an embedded
pseudo-terminal. No protocol layer, no adapters, no JSON-RPC. Just the tools you
already use, side by side, each in its own branch.

## The mental model

dux has three nouns. Once they click, the whole app makes sense.

- **Projects** are git repositories you've added to dux. A project points at a
  checkout on disk and remembers your preferences for it.
- **Agents** are sessions running inside a project. Each agent gets its own git
  worktree on its own branch, so two agents working on the same repo never step on
  each other.
- **Providers** are the CLIs that power agents. Claude, Codex, OpenCode, and Gemini
  ship configured out of the box, and you can wire up any other command yourself.

The flow is: add a project, spin up an agent, pick a provider. dux creates the
worktree, launches the CLI in a real terminal, and tracks the session so you can
walk away and reconnect later.

## The three panes

The window is split into three panes:

- The **left pane** lists your projects and the agent sessions under each one.
- The **center pane** shows the focused agent's live terminal output, or a diff
  view when you want to review changes.
- The **right pane** shows the files an agent has changed, with diffs.

`Tab` and `Shift-Tab` move between panes — that's the primary way you get around.
Every pane has its own local key combinations, and the authoritative list of every
binding lives in the in-app help overlay (press `?`). Everything is rebindable; see
[Configuration](/docs/configuration) for how.

## Where dux keeps its files

dux stores everything in one directory:

- **Linux:** `~/.config/dux/`
- **macOS:** `~/.dux/`

Inside you'll find `config.toml` (your settings), `sessions.sqlite3` (session
state), `dux.log` (logs, the first place to look when something misbehaves), and a
`themes/` directory for any themes you write yourself.

## Where to go next

- [Configuration](/docs/configuration) — the config file, where it lives, and how
  it expands environment variables.
- [Themes](/docs/themes) — switch the look, or build your own.
- [Custom CLI Agents](/docs/custom-agents) — teach dux to drive any CLI you like.
