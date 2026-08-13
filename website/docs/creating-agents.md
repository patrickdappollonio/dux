---
title: Creating agents
description: The four ways to spin up an agent in dux (fresh branch, GitHub PR, existing worktree, or fork) and how provider selection works at creation time.
group: Guides
order: 10
---

An agent in dux is a CLI tool running in its own git worktree on its own branch.
Every agent is isolated: two agents on the same project can work simultaneously
without touching each other's files, and switching between them is instant. Before
you can create agents, you need at least one project added to dux: the Add-project
button in the browser, or the `add-project` command in the terminal UI's palette.
Either one opens a project browser over the same filesystem.

The creation paths below are available from both front ends, and this guide
describes them once for both, because what dux does is the same either way. Only
the way you reach an action differs, so each section names both: in the browser it
is a button or an entry in a row's `⋯` menu, and in the terminal UI it is a command
palette entry. Where a path or a detail exists on only one front end, the text
says so. Palette commands also have default keybindings you can view and
rebind in the in-app help overlay; this guide names the stable command rather
than a key you might have remapped.

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

In the browser, open a project's `⋯` menu and pick **New agent…**. In the terminal
UI, run the `new-agent` palette command and pick a project from the chooser (every
project is listed, including ones with no agents yet). Either way dux inspects that
project's current branch in the background, then opens the naming prompt.

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

The pull is best-effort: it uses `git pull --ff-only` (never a merge or rebase),
it is skipped entirely for repos with no `origin` remote, and a failed pull no
longer blocks creation. If the pull cannot complete, the agent simply starts
from the local branch state and the status message tells you so.

### Copying uncommitted changes

By default, creating an agent also copies the project checkout's uncommitted
and untracked changes into the new worktree, so in-progress work travels with
the agent. Both surfaces have a per-agent checkbox in the naming prompt, and
the default lives in `config.toml`:

```toml
[defaults]
copy_uncommitted_changes_by_default = true
```

The copy is guarded by a same-commit check: changes are copied only when the
project checkout and the new worktree are on the same commit. When they are
not, creation still proceeds and the status message notes that the changes were
not copied. Some things never travel: files matched by `.gitignore`, submodule
and embedded-repository contents, and empty directories.

## Creating an agent from a GitHub PR

In the browser, that is **New agent from PR…** in the New-agent split button's
`⋯` menu (or in a project's own `⋯` menu, to start from that project); in the
terminal UI, the `new-agent-from-pr` palette command. This path is only
available when the `gh` CLI is installed, authenticated
(`gh auth login`), and the `github_integration` setting is enabled (it defaults to
`true`):

```toml
[ui]
github_integration = true
```

dux checks `gh` availability at startup, and again whenever you switch the
integration on, so running `gh auth login` while dux is up is enough: turn the
setting off and on again and dux picks it up without a restart. If `gh` is missing,
or none of the hosts it is logged in to are working, the path is hidden outright on
both front ends: no palette command, no menu entry.

dux asks `gh` for its per-host login status, so one expired login no longer takes
the rest down with it. Signed in to two hosts with a stale token on one of them,
the working host still counts and the GitHub features stay on. On an older `gh`
that cannot report its hosts, dux falls back to a single yes-or-no authentication
check, which is stricter: any host in trouble turns the features off.

**GitHub Enterprise works, on any hostname `gh` is logged in to.** A company
server at `git.company.example` is treated exactly like `github.com` once
`gh auth login --hostname git.company.example` succeeds. That covers both the PR
banner and this from-PR path, and it covers a project's `origin` remote and a PR
URL you paste alike. dux does not judge a host by its name: it counts when the
account `gh` would actually use for it reports success, and it stops counting the
moment that login breaks, whatever the host is called. A `github.`-prefixed
hostname `gh` cannot serve does not qualify either. The exception is an older
`gh` that cannot report its hosts, where dux falls back to recognising
`github.com` and `github.*` only; if your enterprise host is spelled anything
else, upgrade `gh`.

### The reference comes first, and dux works out the project

Open this from the global command (the `new-agent-from-pr` palette entry in the
terminal UI, **New agent from PR…** in the browser's New-agent menu) and the
first thing you see is the reference field. No project is chosen and none is
asked for: paste the link and dux compares the repository it names against every
project you have, then takes you to the right one.

Three things can happen:

- **One project is a checkout of that repository.** dux goes straight on to
  resolve the pull request and name the agent.
- **Two or more are** (you keep the same repository checked out twice). dux
  shows you just those and asks which one this agent belongs in.
- **None is.** dux tells you which repository it could not find a project for and
  offers the project picker so you can point at a checkout you already have on
  disk. **dux will not clone a repository it does not have.** Every way of adding
  a project takes a directory that already exists; if the repository is not on
  this machine yet, clone it yourself and add it as a project first.

Some projects cannot be compared at all: the directory is gone, git cannot read
an `origin`, or the address is on a host `gh` is not signed in to. Those are not
answers, they are unknowns, so dux says so rather than telling you no project has
the repository. If the message mentions projects it could not check, one of them
may well be the checkout you were after.

If you would rather start from a project, there is a secondary action under the
field, "choose an existing project", that opens the project selector and puts the dialog back
into project-first mode. Anything you have already typed comes with you. Opening
this path from a project's own `⋯` menu (browser) skips straight to that mode,
exactly as it always did.

### What the field accepts

It is generous on purpose, because you are pasting from a browser bar, a chat
message, or memory. Every one of these names the same repository:

```
example/application
github.com/example/application
git@github.com:example/application.git
https://github.com/example/application
https://github.com/example/application/issues
https://github.com/example/application/security/dependabot
```

A trailing path is a browser route and is ignored, so a link copied off the Files
or Commits tab of a pull request works as-is. On top of those, the pull-request
spellings:

```
https://github.com/example/application/pull/123
example/application#123
#123
123
```

`example/application#123` deliberately names **no host**, and dux does not assume
github.com for it: it looks for that repository across all your projects on
whatever host each one is on, so it finds your company server's checkout if that
is the only one you have.

A number on its own (`#123` or `123`) is the one form that needs a project
already chosen, because by itself it does not say which repository it is in. With
no project, dux refuses it and points you at the "choose an existing project"
action; with a project chosen it behaves exactly as it always has.

An address with a scheme is read by the same rules a browser uses, so
`https://github.com/acme/widget/../gadget` names `acme/gadget`, exactly as it
would if you pasted it into the address bar, and percent escapes are decoded.
A scheme dux does not speak is refused rather than guessed at.

This leniency applies only to what **you** type. A project's own `origin` address
is read from git, and dux reads it by git's rules alone, where a trailing path is
part of the address rather than a route to ignore.

Resolution reads each project's `origin` fresh every time, in a background
worker. There is no cache, so editing a remote, changing a git `insteadOf`
rewrite, or repairing a broken address takes effect immediately.

### Naming and fetching

Once the pull request is resolved, dux asks you to confirm or edit the branch
name, pre-filled with the PR's head branch name. In the terminal UI that is a
second prompt after the PR reference; in the browser the PR reference and the
name are two fields in one dialog. The name you confirm is what
the fetch targets, so on confirmation dux:

1. Fetches the PR's head ref into that local branch using
   `git fetch origin pull/<number>/head:refs/heads/<branch>`.
2. Creates a worktree on that branch.

If the branch already exists locally (for example, from a previous fetch), dux
attaches to it without fetching again.

### How PR status stays fresh

Once `github_integration` is on, dux shows a PR status pill on each agent branch
and keeps it current without hammering GitHub's API. Updates are driven by events:
pushing to a branch refreshes that agent's PR, and bringing an agent to the
foreground refreshes it too. A slow background poll is the only fallback, for
changes made on GitHub itself (someone merges or closes a PR in the browser):

```toml
[ui]
# Seconds between blind PR-status safety polls. Most updates come from events,
# so this is just the backstop. Set to 0 to rely on events alone.
pr_poll_interval_seconds = 180
```

Each poll batches your tracked PRs into as few GraphQL requests as possible — one
per GitHub host, up to ~100 PRs each — so the cost to your API quota stays low
even with many agents. If your quota ever runs low (or GitHub starts erroring),
dux pauses PR checks until it recovers, and tells you: a status line in the
terminal UI, a toast in the browser.

## Creating an agent from an existing worktree

In the browser, that is **Worktrees…** in a project's `⋯` menu (or **New agent
from existing worktree…** in the app menu, which asks for the project first);
in the terminal UI, the `new-agent-from-worktree` palette command, then a
project from the chooser. dux opens a picker that lists every git worktree it finds
for that project's repository. Worktrees are grouped into two categories:

- **Managed worktrees**: worktrees already under dux's `worktrees/` directory.
  If one has no agent yet, dux attaches a new session to it without touching the
  branch or files.
- **External worktrees** (terminal UI only): worktrees that exist in the
  repository but live outside dux's managed directory (for example, one you
  created with `git worktree add` yourself). dux forks these: it creates a new
  managed worktree branched from the external worktree's current `HEAD` commit
  and copies any dirty and untracked files across (gitignored files do not
  travel) so you don't lose in-progress work.

The browser's **Worktrees** dialog lists managed worktrees only. To adopt an
external one, use the `new-agent-from-worktree` palette command in the terminal
UI. The browser dialog is also a small manager: an unused worktree's `⋯` menu
offers **Delete worktree…**, which removes the directory from disk after a
confirmation that names the branch, names the full path, and says specifically
when there are uncommitted changes to lose. That confirmation also offers to
delete the branch, ticked by default; untick it and the branch survives. The
project picker in front of it labels each project with how many worktrees it
has, so an empty project is a choice rather than a surprise, and the dialog
offers **Back** to return to that list.

The main checkout itself is not selectable; dux keeps that for you to work in
outside of agent sessions.

Worktrees that already have an agent are shown but cannot be selected. In the
terminal UI, selecting one reports "That worktree already has an agent."; in the
browser the row is disabled and its tooltip explains why.

## Forking an existing agent

Forking starts from an existing agent rather than a project. In the browser, open
that agent's `⋯` menu and pick **Fork agent…**; in the terminal UI, select the agent
in the left pane and run the `fork-agent` palette command.
Forking creates a brand-new worktree branched from the source agent's current
`HEAD` commit, then copies the uncommitted and untracked changes across so the
fork starts where the original agent is right now. Files matched by
`.gitignore`, submodule and embedded-repository contents, and empty directories
do not travel (nor do edits hidden with `assume-unchanged` or `skip-worktree`,
which are invisible to git status).

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

All three levels are editable from either front end. In the terminal UI they are the
`change-default-provider`, `change-project-default-provider`, and
`change-agent-provider` palette commands. In the browser, the global default is the
**Default provider for new agents** row in **Preferences…**, a project's default
lives in **Project settings…** on its `⋯` menu, and an agent's provider in **Change
agent provider…** on its own `⋯` menu (a single tab is retargeted with **Change
provider…** on the tab's `⋯` menu). Swapping an agent's provider never yanks a running
session out from under itself; it takes effect the next time that tab launches.

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

You can toggle every level without editing the file directly. In the terminal UI,
`toggle-project-auto-reopen-agents` flips the selected project's setting and
`toggle-agent-auto-reopen` flips a single agent's. In the browser, the global switch
is a **Preferences…** row, the project one is in **Project settings…**, and an
agent's own is **Enable/Disable agent auto-reopen** on its `⋯` menu. Changes take
effect the next time dux starts.

If an agent's provider command is not found when dux tries to reopen it, the
worktree is left intact and the error is reported; the agent
appears in the list and you can reconnect it manually once the CLI is available.
