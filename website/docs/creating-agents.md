---
title: Creating agents
description: The four ways to spin up an agent in dux (fresh branch, GitHub PR, existing worktree, or fork) and how provider selection works at creation time.
group: Guides
order: 10
---

An agent in dux is a CLI tool running in its own git worktree on its own branch.
Every agent is isolated: two agents on the same project can work simultaneously
without touching each other's files, and switching between them is just a
keystroke. Before you can create agents, you need at least one project added to
dux (see the project browser, accessible via the `add-project` palette command).

Every action below is reachable from the command palette. Each also has a
default keybinding you can view (and rebind) in the in-app help overlay (`?`),
so this guide names the stable palette commands rather than keys that you might
have remapped.

## The mental model

When you create an agent, dux does three things in a background worker so the UI
stays responsive:

1. Creates (or attaches to) a git worktree for the chosen branch.
2. Runs your project's [`startup_command`](/docs/startup-commands), if one is configured.
3. Launches the provider CLI inside that worktree in a pseudo-terminal.

Worktrees live in dux's data directory, under a `worktrees/` subdirectory:

- **Linux:** `~/.config/dux/worktrees/<project-name>/<branch-name>/`
- **macOS:** `~/.dux/worktrees/<project-name>/<branch-name>/`

Because each agent owns a real git worktree, your project's `.gitignore`, git
hooks, and local config all behave exactly as they do in the main checkout.

## Naming an agent

Every creation path ends at a naming prompt. dux uses the branch name as the
agent name: it becomes a git branch, so only ASCII letters, digits, `-`, `_`,
and `/` are accepted. Spaces are transparently converted to dashes.

If you leave the field empty and the `enable_randomized_pet_name_by_default`
setting is on, dux generates a two-word Docker-style pet name (for example,
`brave-morse`) for both the agent name and the branch. You can toggle this
behaviour with the checkbox in the naming prompt or permanently in `config.toml`:

```toml
[defaults]
enable_randomized_pet_name_by_default = false
```

## Creating a new agent from scratch

Select a project in the left pane and run the `new-agent` palette command. dux
inspects the project's current branch in the background, then opens the naming
prompt.

On confirmation, dux runs `git worktree add -b <name> <path> <leading-branch>`,
branching from the project's leading branch. If the name you entered matches an
existing local branch, dux asks whether to attach to that branch instead of
creating a new one, which is useful when you want to continue work that already
started.

### Pulling before create

By default dux pulls the leading branch before creating the worktree, so the new
agent starts from the freshest upstream commit. You can change the default in
`config.toml`:

```toml
[defaults]
pull_before_creating_agent_by_default = true
```

The pull only proceeds if the source checkout has no uncommitted changes; if it
does, creation fails with a clear error rather than silently clobbering your work.

## Creating an agent from a GitHub PR

Select a project and run the `new-agent-from-pr` palette command. This path is
only available when the `gh` CLI is installed, authenticated (`gh auth login`),
and the `github_integration` setting is enabled (it defaults to `true`):

```toml
[ui]
github_integration = true
```

dux checks `gh` availability at startup. If it is missing or not authenticated,
the `new-agent-from-pr` command is hidden from the palette entirely.

When you trigger the command, dux opens a prompt where you can paste a GitHub PR
URL or type a PR number. After you confirm, dux:

1. Fetches the PR's head ref into a local branch using
   `git fetch origin pull/<number>/head:refs/heads/<branch>`.
2. Creates a worktree on that branch.
3. Opens the naming prompt (pre-filled with the PR's head branch name).

If the branch already exists locally (for example, from a previous fetch), dux
attaches to it without fetching again.

## Creating an agent from an existing worktree

Select a project and run the `new-agent-from-worktree` palette command. dux opens
a picker that lists every git worktree it finds for that project's repository.
Worktrees are grouped into two categories:

- **Managed worktrees**: worktrees already under dux's `worktrees/` directory.
  If one has no agent yet, dux attaches a new session to it without touching the
  branch or files.
- **External worktrees**: worktrees that exist in the repository but live
  outside dux's managed directory (for example, one you created with
  `git worktree add` yourself). dux forks these: it creates a new managed
  worktree branched from the external worktree's current `HEAD` commit and copies
  any dirty and untracked files across so you don't lose in-progress work.

The main checkout itself is not selectable; dux keeps that for you to work in
outside of agent sessions.

Worktrees that already have an active agent are shown in the picker but cannot be
selected; the error "That worktree already has an agent." is shown if you try.

## Forking an existing agent

Select an agent in the left pane and run the `fork-agent` palette command.
Forking creates a brand-new worktree branched from the source agent's current
`HEAD` commit, then copies the entire
working tree across (including uncommitted edits) so the fork starts in the
exact same state the original agent is in right now.

This is useful for exploring two different approaches to the same problem: fork
the agent at the decision point and let each branch go its own way.

## Choosing a provider at creation time

Every agent is tied to one provider. At creation time, dux uses whichever
provider is configured as the default for that project:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
default_provider = "claude"
```

If no project-level default is set, dux falls back to the global default defined
in `[defaults]`:

```toml
[defaults]
provider = "claude"
```

You can change the global default at any time with the `change-default-provider`
palette command, or change just one project's default with
`change-project-default-provider`. To swap the provider on a specific existing
agent after creation, use `change-agent-provider`.

## Auto-reopening agents on startup

Agents are persistent. When you quit dux and reopen it, agents can resume
automatically if `auto_reopen_agents` is enabled. The setting lives at two levels:

```toml
# Global default: applies to all projects unless overridden
[ui]
auto_reopen_agents = false

# Per-project override stored in config.toml
[[projects]]
id   = "a4f3..."
auto_reopen_agents = true
```

You can toggle either level without editing the file directly: use
`toggle-project-auto-reopen-agents` from the palette to flip the selected
project's setting, or `toggle-agent-auto-reopen` to flip a single agent's
behaviour. Changes take effect the next time dux starts.

If an agent's provider command is not found when dux tries to reopen it, the
worktree is left intact and the error is shown in the status bar; the agent
appears in the list and you can reconnect it manually once the CLI is available.
