---
title: Supported agents
description: The four agents dux ships wired in, what each one can and cannot do inside dux, and how to add any other CLI.
group: Getting started
order: 1.5
---

dux runs an agent the way you would in a terminal: it starts the CLI in your agent's
working copy and shows you its screen. Four agents come wired in, with a provider entry
each in `config.toml`, and `claude` is the default for new agents until you pick another.

> [!TIP]
> dux works with any agent. If a CLI runs in a terminal, dux can run it: add a
> `[providers.<name>]` block with its `command` to `config.toml` and it shows up in the
> place dux lets you pick a provider, like the four below. No adapters, no plugins. See
> [Custom CLI agents](/docs/custom-agents).

## What every agent gets

The same treatment, whichever CLI is behind the agent:

- Its own git worktree and branch, or a folder you already have for a standalone agent.
- The mouse wheel forwarded to it whenever it asks for the mouse, and page keys when it
  takes over the whole screen.
- Files dropped or pasted onto its pane land in the worktree and the path is typed for it.
- Several tabs in one worktree, each its own provider session with its own conversation.

Where the agents differ is in resuming, in how they announce that they need you, and in
how they read a pasted path. That is what the table and notes below cover.

| Agent | Command | Resumes its conversation | Attention signal |
|---|---|---|---|
| Claude Code | `claude` | Yes, per worktree | Yes, out of the box |
| Codex | `codex` | Yes, per worktree | After a one-line setting |
| OpenCode | `opencode` | Yes, per worktree | No signal today |
| Copilot | `copilot` | Never | Progress yes, finished turn after a setting |

When an agent's CLI is missing, dux says so and prints its install line for you:

- **Claude Code:** `curl -fsSL https://claude.ai/install.sh | bash`
- **Codex:** `brew install --cask codex`
- **OpenCode:** `curl -fsSL https://opencode.ai/install | bash`
- **Copilot:** `curl -fsSL https://gh.io/copilot-install | bash`

## Claude Code

The default provider.

- **Resume:** reopen a stopped agent and Claude continues the conversation it had in
  that worktree.
- **Needs-you indicator:** works with no setup. If yours stays dark, set
  `preferredNotifChannel: "terminal_bell"` in Claude's own settings.
- **Pasted paths:** handed over exactly as written. Claude strips one pair of quotes and
  then unescapes, so quoting would only corrupt an apostrophe in the path.

## Codex

- **Resume:** per worktree, on a recent enough Codex build.
- **Needs-you indicator:** set `tui.notification_method` in Codex's config to any value
  to light up dux's attention indicators.
- **Pasted paths:** single-quoted, because Codex reads its input like a shell and takes
  only one token: a bare path with a space in it would fail silently.
- **Attachment length:** Codex accepts at most 1000 characters in one attachment. Past
  that, dux still saves the file and shows you its full path to reference yourself,
  rather than sending something Codex would silently ignore.

## OpenCode

- **Resume:** per worktree. If resuming hangs before OpenCode shows anything, dux waits
  three seconds, then starts it fresh instead of leaving you with a blank screen.
- **Needs-you indicator:** none that dux can see today, because its notifications go
  through plugins. Its rows never light up on their own; when a future version rings a
  bell, dux picks it up with no change on your side.
- **Pasted paths:** go in as written.

## Copilot

> [!IMPORTANT]
> Copilot never resumes. Its own `--continue` picks up the most recent conversation from
> anywhere on your machine, not the one that belongs to this worktree, so resuming could
> reattach to a different project's conversation. Every Copilot tab starts fresh.

- **Progress:** Copilot reports it on its own, so the working spinner is accurate with
  no setup.
- **Needs-you indicator:** its finished-turn bell ships switched off (since v1.0.60), so
  turn the terminal bell back on in Copilot's config if you want a completed turn
  flagged.
- **Pasted paths:** go in as written. Copilot is closed source, so that form is the safe
  default rather than a measured one, and `web_dragdrop_paste` in its provider block
  changes it if your version wants quoting.

## Changing any of this

Every value above is a setting in the provider's `[providers.<name>]` block: the command
and its arguments, the resume arguments, how a pasted path is quoted, and whether scrolling
is forwarded. The config file explains each key inline, and
[Custom CLI agents](/docs/custom-agents) walks through adding a new one.
