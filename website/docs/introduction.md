---
title: Introduction
description: What dux is, the mental model behind it, and how the pieces fit together.
group: Getting started
order: 1
---

`dux` runs multiple AI coding agents in parallel, usually one git worktree each, and
straight in a folder you already have when you want that instead. It spawns the real
CLI for each agent (Claude Code, Codex, Copilot, OpenCode, or anything else you can run
in a terminal) inside an embedded pseudo-terminal. No protocol layer, no adapters, no
JSON-RPC. Just the tools you already use, side by side, each in its own branch.

## Two front ends, one engine

dux has a terminal UI and a web UI. Both are first class, and both share the same
projects, agents, worktrees and config file, so an agent you start in one is the same
agent in the other. They differ on purpose:

- The **terminal** gives you full keyboard control, rebindable keys, a command palette
  and themes.
- The **browser** gives you reach: any device on your network, a phone included, plus
  editing files in the page and desktop notifications.

To know whether something is available where you are, ask the surface. The terminal's
help overlay and command palette list what it can do; the browser's cog menu and row
menus list what it can do.

The web UI is [server mode](/docs/server-mode), started with `dux server`, flipped on
from a running terminal UI, or served quietly in the background of one. It is always
one dux process, so moving between the front ends is a hand-off rather than a second
copy, and your agents keep running either way.

## The mental model

dux has three nouns:

- **Projects** are git repositories you have added to dux. A project points at a
  checkout on disk and remembers your preferences for it.
- **Agents** are sessions running inside a project. Each one gets its own git worktree
  on its own branch, so two agents on the same repo never step on each other. A
  **standalone agent** is the exception you ask for by name: it runs in a folder you
  already have, with no branch and no worktree of dux's, and dux never creates, moves
  or removes that folder. See [Creating agents](/docs/creating-agents).
- **Providers** are the CLIs that power agents. Claude, Codex, OpenCode, and Copilot
  ship configured out of the box, and you can wire up any other command yourself.

The flow is: add a project, spin up an agent, pick a provider. dux creates the worktree,
launches the CLI in a real terminal, and tracks the session so you can walk away and
reconnect later.

## The three panes

Both front ends lay the workspace out the same way:

- The **left pane** lists your agents in a single flat list, most-active first, with a
  search filter and a project chooser.
- The **center pane** shows the focused agent's live terminal, and it is a real typing
  surface on both front ends: focus it and your keystrokes go to the agent, while dux's
  own shortcuts keep working around them. The terminal UI adds a fullscreen toggle that
  passes input through verbatim, and its center pane doubles as the diff view; in the
  browser a diff opens in the file-editor overlay instead.
- The **right pane** shows the files an agent has changed, with diffs.

> [!IMPORTANT]
> In the terminal UI's windowed layout, not every key reaches the agent. dux keeps its
> own chords for itself: pane navigation, the command palette, tab switching, and the
> rest of its bindings fire in dux instead of typing into the agent. Fullscreen forwards
> everything verbatim, so it is the escape hatch when the agent needs a key dux normally
> keeps.

> [!TIP]
> Every dux binding is configurable under `[keys]`, so you can free a specific chord for
> the agent while windowed; see [Configuration](/docs/configuration#keybindings). The
> `input-debugging` palette command shows exactly what dux receives for each keypress,
> which is what you want open while crafting a binding.

In the terminal UI, focus-next and focus-previous keys move between panes, and every
pane has its own local key combinations; the in-app help overlay is the authoritative
list. In the browser the same three panes are click-driven, with a collapsible sidebar
and a resizable Changes split; see
[The workspace in the browser](/docs/web-workspace).

## Where dux keeps its files

dux stores everything in one directory:

- **Linux:** `~/.config/dux/`
- **macOS:** `~/.dux/`

Inside are `config.toml` (your settings), `sessions.sqlite3` (session state), `dux.log`
(the first place to look when something misbehaves), a `themes/` directory for themes
you write, and `worktrees/`, which holds the checkouts your agents work in.

> [!NOTE]
> That directory is **yours alone**. dux makes it owner-only on every startup, and sets
> owner-only permissions on the files it manages. Your config and session database can
> hold environment variables you set for a project, which is exactly where an API token
> tends to live, so no other user on the machine gets to read them. If you are upgrading
> from a world-readable directory, the next start tightens it. That pass only ever
> removes group and world access, never your own.

> [!WARNING]
> Making `config.toml` read-only does **not** stop dux saving over it. A save writes a
> new file and renames it into place, which needs write permission on the directory
> rather than on the file, so a read-only config is replaced anyway and comes back
> owner-writable. If you need a config dux cannot touch, keep it somewhere dux does not
> write to.

dux never follows a symlink when setting a mode, so a `config.toml` that links into a
dotfiles repository leaves that repository's file exactly as it is. A mode dux cannot
set is a warning, not an error, so a directory on a volume without `chmod` still works
and dux still starts.

## Where to go next

- [Server mode overview](/docs/server-mode): the web UI, how to start it, and the
  no-login trust model to understand before you expose it.
- [The workspace in the browser](/docs/web-workspace): the browser layout, its
  terminals, and the phone experience.
- [Reaching dux over Tailscale](/docs/tailscale): open your workspace on your phone,
  and the caveats that come with it.
- [Configuration](/docs/configuration): the config file, where it lives, and how it
  expands environment variables.
- [First run & what's new](/docs/first-run-and-whats-new): the welcome screen, the
  what's-new screen, and how to turn either automatic screen off.
- [Managing Themes](/docs/themes): switch the look, or build your own.
- [Custom CLI Agents](/docs/custom-agents): teach dux to drive any CLI you like.
