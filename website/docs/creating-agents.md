---
title: Creating agents
description: The ways to spin up an agent in dux (fresh branch, GitHub PR, existing worktree, fork, or a plain folder you already have) and how provider selection works at creation time.
group: Guides
order: 10
---

An agent in dux is usually a CLI tool running in its own git worktree on its own branch.
Two agents on the same project work at the same time without touching each other's files,
and switching between them is instant.

First you need a project: the Add-project button in the browser, or the `add-project`
command in the terminal UI's palette. Either opens a project browser over the same
filesystem.

One path skips all of that. A **standalone agent** has no branch and no worktree and runs
in a folder you point it at, with no project at all. See
[Running an agent in a folder you already have](#running-an-agent-in-a-folder-you-already-have).

Every path below exists on both front ends and does the same thing on each. Only the way
you reach it differs, so each section names both: a button or a row's `⋯` menu in the
browser, a palette command in the terminal UI.

## The mental model

Creating an agent does three things:

1. Creates, or attaches to, a git worktree for the chosen branch.
2. Runs your project's [`startup_command`](/docs/startup-commands), if one is configured.
3. Launches the provider CLI inside that worktree.

Worktrees live under a `worktrees/` subdirectory of dux's data directory:

- **Linux:** `~/.config/dux/worktrees/<project-name>/<branch-name>/`
- **macOS:** `~/.dux/worktrees/<project-name>/<branch-name>/`

Because each agent owns a real git worktree, your project's `.gitignore`, git hooks, and
local config behave exactly as they do in the main checkout.

## Naming an agent

Every creation path that makes a branch ends at a naming prompt, and there the name IS
the branch name. It becomes a git ref, so only ASCII letters, digits, `-`, `_`, and `/`
are accepted, and spaces become dashes.

Tick the pet-name checkbox in the naming prompt and dux generates a two-word pet name
such as `brave-morse`, for both the agent and the branch. The checkbox starts unticked;
to make pet names the default for every new agent, turn it on permanently:

```toml
[defaults]
enable_randomized_pet_name_by_default = true
```

A standalone agent has no branch, so its name is a plain label taken exactly as you type
it, punctuation included. Leave it empty (at creation, or when renaming later) and dux
names it after the folder rather than
inventing a pet name.

This is the naming prompt in the terminal UI, with the pet-name checkbox ticked and the
generated name filled in:

![The terminal UI naming prompt for a new agent, with a generated pet name in the field, a ticked pet-name checkbox, and a ticked checkbox for copying uncommitted changes.](/screens/tui-name-new-agent.png)

## Creating a new agent from scratch

In the browser, open a project's `⋯` menu and pick **New agent…**. In the terminal UI,
run `new-agent` and pick a project from the chooser (every project is listed, including
ones with no agents yet). Either way dux checks that project's
current branch, then opens the naming prompt.

The terminal UI's chooser lists every project, how many agents each one has, and where it
lives, and its footer carries the way out to a standalone agent:

![The terminal UI project chooser for a new agent, listing two projects with their agent counts and paths, and a footer key for creating a standalone agent instead.](/screens/tui-new-agent-chooser.png)

On confirmation dux creates a worktree on a new branch, branched from the project's
leading branch. If the name matches an existing local branch, dux asks whether to attach
to that branch instead, which is what you want when continuing work that already started.

> [!IMPORTANT]
> Attaching matters at the other end of the agent's life. dux remembers that the branch
> existed first, so deleting the agent never deletes it. The worktree goes if you ask for
> it; the branch stays, because it was yours before the agent was.

### Pulling before create

By default dux pulls the leading branch first, so the new agent starts from the freshest
upstream commit:

```toml
[defaults]
pull_before_creating_agent_by_default = true
```

The pull is best-effort. It is fast-forward only, never a merge or rebase, it is skipped
for repos with no `origin` remote, and a failed pull does not block creation: the agent
starts from the local branch state and the status message says so.

### Copying uncommitted changes

By default, creating an agent copies the project checkout's uncommitted and untracked
changes into the new worktree, so in-progress work travels with the agent. Both surfaces
have a per-agent checkbox in the naming prompt, and the default lives in config:

```toml
[defaults]
copy_uncommitted_changes_by_default = true
```

Changes are copied only when the project checkout and the new worktree are on the same
commit. When they are not, creation still proceeds and the status message notes that
nothing was copied. Files matched by `.gitignore`, submodule and embedded-repository
contents, and empty directories never travel.

## Creating an agent from a GitHub PR

In the browser, **New agent from PR…** in the launcher's `⋯` menu at the bottom of the
sidebar, under **Agents**, or in a project's own `⋯` menu to start from that project. In
the terminal UI, the `new-agent-from-pr` palette command.

This path needs the `gh` CLI installed, authenticated with `gh auth login`, and
`github_integration` on, which it is by default:

```toml
[ui]
github_integration = true
```

dux checks `gh` at startup and again whenever you switch the integration on, so running
`gh auth login` while dux is up is enough: toggle the setting off and on. If `gh` is
missing or none of its logins work, the path is hidden outright on both front ends. One
expired login does not take the others down: if you are signed in to two hosts and one
token is stale, the working host keeps the GitHub features on.

**GitHub Enterprise works, on any hostname `gh` is logged in to.** A company server at
`git.company.example` is treated exactly like `github.com` once
`gh auth login --hostname git.company.example` succeeds, for the PR banner and for this
path. An older `gh` that cannot report its hosts falls back to a single yes-or-no login
check, which is stricter (any host in trouble switches the GitHub features off), and to
recognising `github.com` and `github.*` only; if your enterprise host is spelled anything
else, upgrade `gh`.

### The reference comes first, and dux works out the project

Open this from the global command and the first thing you see is the reference field. No
project is asked for: paste the link and dux compares the repository it names against
every project you have.

- **One project is a checkout of that repository.** dux goes straight on to resolve the
  pull request and name the agent.
- **Two or more are.** dux shows you just those and asks which one this agent belongs in.
- **None is.** dux names the repository it could not place and offers the project picker.

> [!IMPORTANT]
> **dux will not clone a repository it does not have.** Every way of adding a project
> takes a directory that already exists. If the repository is not on this machine yet,
> clone it yourself and add it as a project first.

Some projects cannot be compared at all: the directory is gone, git cannot read an
`origin`, or the address is on a host `gh` is not signed in to. dux reports those as
unknowns rather than claiming no project has the repository, so if the message mentions
projects it could not check, one of them may be the checkout you wanted.

To start from a project instead, use the "choose an existing project" action under the
field. Anything you have typed comes with you. Opening this path from a project's own `⋯`
menu in the browser starts in that mode.

### What the field accepts

It is generous on purpose, because you are pasting from a browser bar, a chat message, or
memory. Every one of these names the same repository:

```
example/application
github.com/example/application
git@github.com:example/application.git
https://github.com/example/application
https://github.com/example/application/issues
https://github.com/example/application/security/dependabot
```

A trailing path is a browser route and is ignored, so a link copied off the Files or
Commits tab of a pull request works as-is. On top of those, the pull-request spellings:

```
https://github.com/example/application/pull/123
example/application#123
#123
123
```

`example/application#123` names **no host**, and dux does not assume github.com for it:
it looks for that repository across all your projects on whatever host each one is on, so
it finds your company server's checkout if that is the only one you have.

A number on its own, `#123` or `123`, is the one form that needs a project already chosen,
because by itself it does not say which repository it is in. With no project, dux refuses
it and points you at "choose an existing project".

An address with a scheme is read by the same rules a browser uses, so
`https://github.com/acme/widget/../gadget` names `acme/gadget`, and percent escapes are
decoded. A scheme dux does not speak is refused. This leniency applies only to what
**you** type: a project's own `origin` is read by git's rules, where a trailing path is
part of the address.

Each project's `origin` is read fresh every time, so editing a remote, changing a git
`insteadOf` rewrite, or repairing a broken address takes effect immediately.

### Naming and fetching

Once the pull request resolves, dux asks you to confirm or edit the branch name,
pre-filled with the PR's head branch. In the terminal UI that is a second prompt; in the
browser the reference and the name are two fields in one dialog. The name you confirm is
what the fetch targets: dux fetches the PR's head ref into that local branch, then
creates a worktree on it.

If the branch already exists locally, from a previous fetch say, dux attaches to it
without fetching again, and deleting the agent later leaves that branch alone. A branch
dux fetched for you is dux's own, so that one is cleaned up with the agent.

### How PR status stays fresh

With `github_integration` on, dux shows a PR status pill on each agent branch. Updates
are event-driven: pushing to a branch refreshes that agent's PR, and bringing an agent to
the foreground refreshes it too. A slow background poll is the fallback, for changes made
on GitHub itself:

```toml
[ui]
# Seconds between blind PR-status safety polls. Most updates come from events,
# so this is just the backstop. Set to 0 to rely on events alone.
pr_poll_interval_seconds = 180
```

When a branch name is reused, dux follows the most recent pull request on it, preferring
one that is open.

If your GitHub API quota runs low, or GitHub starts erroring, dux pauses PR checks until
it recovers and tells you: a status line in the terminal UI, a toast in the browser.

Above or below the agent's terminal, a one-line banner carries the pull request's number,
its state and its title, on both surfaces. Clicking the banner opens that pull request in
your browser, wherever you click it. In the terminal UI the `open-current-pr` command does
the same from the keyboard, and it is the way in while an agent pane is maximized, since
the maximized surface covers the banner (it also steps aside while you are typing into the
pane, where the same command is the way in). Which side of the terminal the banner sits
on is the `pr_banner_position` setting under `[ui]`.

## Creating an agent from an existing worktree

In the browser, **Worktrees…** in a project's `⋯` menu, or **New agent from existing
worktree…** in the app menu, which asks for the project first and offers **Back** to
return to that list. In the terminal UI, the
`new-agent-from-worktree` palette command. Either opens a picker of every git worktree
for that project's repository, in two groups:

- **Managed worktrees**, already under dux's `worktrees/` directory. One with no agent
  yet gets a new session attached without touching the branch or files. An adopted
  worktree's branch came with it, so deleting that agent removes the worktree and keeps
  the branch.
- **External worktrees** (terminal UI only), which exist in the repository but live
  outside dux's managed directory, such as one you created with `git worktree add`. dux
  forks these: a new managed worktree branched from the external worktree's current
  `HEAD`, with dirty and untracked files copied across. Gitignored files do not travel.

The main checkout is never selectable; dux keeps that for you. Worktrees that already
have an agent are shown but disabled, with a tooltip explaining why; in the terminal UI,
selecting one reports "That worktree already has an agent."

### Deleting a worktree, and its branch

The browser's **Worktrees** dialog doubles as a manager: an unused worktree's `⋯` menu
offers **Delete worktree…**. The terminal UI has the same manager as the
`manage-worktrees` palette command. The project picker in front of it labels each project
with how many worktrees it has.

This is the terminal UI's manager: free worktrees on top, the ones a live agent is holding
below them, each row naming its branch and saying whether there is uncommitted work in it.

![The terminal UI worktree manager listing one removable worktree and two held by an agent, each with its branch and an uncommitted-changes note.](/screens/tui-worktree-manager.png)

> [!CAUTION]
> Deleting a worktree removes the directory from disk. The confirmation names the branch
> and the full path, and says specifically when there are uncommitted changes to lose. It
> also offers to delete the branch, **ticked by default**; untick it and the branch
> survives. If git refuses the deletion, dux reports the branch as still there with git's
> own reason.

![The terminal UI confirmation for deleting a worktree, naming the path, warning about the uncommitted changes, and offering a ticked checkbox that also deletes the branch.](/screens/tui-worktree-delete-confirm.png)

Worktrees a live agent is holding are listed but unselectable: removing one from under a
running session leaves it broken. Delete the agent instead.

Either manager is the manual override for deleting a branch dux is keeping, for as long as
its worktree is still there. Deleting an agent only ever deletes branches dux created.
Once the worktree is gone the manager can no longer reach the branch, and `git branch -D`
is the way.

## Forking an existing agent

Forking starts from an existing agent rather than a project. In the browser, that agent's
`⋯` menu and **Fork agent…**; in the terminal UI, select the agent and run `fork-agent`.

dux creates a new worktree branched from the source agent's current `HEAD`, then copies
the uncommitted and untracked changes across, so the fork starts where the original is
right now. Fork at a decision point to explore two approaches to the same problem.

> [!WARNING]
> Files matched by `.gitignore`, submodule and embedded-repository contents, and empty
> directories do not travel. Neither do edits hidden with `assume-unchanged` or
> `skip-worktree`, which are invisible to git status.

## Running an agent in a folder you already have

Everything above creates a branch and a working copy. A **standalone agent** does not:
you pick a folder, and the AI runs there. Good for a scratch directory, a notes folder, a
pile of downloads to sort, or a repository you want worked on in place.

This is the browser.

![A standalone agent in the sidebar showing the folder it runs in, with the changes panel saying the folder has no git repository.](/screens/sidebar-standalone.png)

And this is the terminal UI, where the star and the folder replace the project a managed
agent would name, and the changes panel says why it has nothing to show.

![The terminal UI with a standalone agent selected: its row carries a star over the folder path, and the changes panel says the folder has no git repository.](/screens/tui-standalone-star.png)

In the browser, the launcher's `⋯` menu and **New standalone agent…**. In the terminal
UI there are three ways in: a key anywhere in the agents pane, a key inside the "New
agent in project" chooser too (so you can change your mind once you are already there and
no project fits), and the `new-standalone-agent` palette command. The `?` help overlay
names the keys, and you can rebind them under `[keys]` in `config.toml`. Every way in
opens the same folder browser. Any folder is accepted: it does not have to be a git
repository, and dux initializes nothing in it.

> [!NOTE]
> Once you start filtering inside the chooser, the key that still works is a modifier
> chord, because plain letters go into the search box. GNU Screen's flow control can
> swallow that chord before dux ever sees it; the palette command still works there, and
> so does rebinding the key to something your terminal passes through.

What a standalone agent does NOT have:

- **No branch and no worktree.** dux creates nothing on disk for it.
- **No project.** It sits among your other agents, told apart by the `✷` star over its
  folder on the row's second line. A standalone terminal wears the same star.
- **No branch features.** Pushing, pulling, forking, pull requests and branch renaming
  are about a branch dux manages, so those actions are absent rather than offered and
  refused.
- **No startup command and no project environment.** Both are project-scoped. A
  standalone agent gets your global environment and nothing layered on top.

Everything else is there: the embedded terminal, agent tabs, companion terminals, the
in-browser editor, file drops, renaming, the resource monitor and auto-reopen.

### dux never creates, moves or removes the folder

> [!IMPORTANT]
> The folder's existence and location are yours alone. dux does not create it, move it or
> remove it, ever. Deleting the agent removes dux's own record and nothing else, and the
> delete dialog says so: there is no "also remove the worktree" checkbox, because there
> is no worktree. A factory reset skips it too.

Things do get written *inside* it: the agent works in it, a dropped file lands in it, a
commit writes to its repository.

A file you drop onto a standalone agent is saved in a hidden upload directory inside the
folder. Because dux never cleans the folder up, that directory stays there after the agent
is gone. Remove it yourself if you do not want it.

### The changes panel follows the folder

With no branch, the changed-files panel is driven by the folder. When the folder is
itself a git repository's top level, the panel works as it does anywhere: changed files,
diffs, staging, committing. Pushing stays out, because it publishes a branch. A commit
made from the panel runs that repository's own git hooks.

When the folder is not a repository the panel is quiet, and it says which quiet it is:

- The folder has no git repository at all.
- The folder sits **inside** a repository rooted somewhere else.
- dux could not consult git. Nothing is guessed and no change is written.

> [!WARNING]
> The middle case is quiet on purpose. Git answers questions by walking up parent
> directories, so showing changes there would show, stage and commit to that other
> repository. Point an agent at the repository's top level, or add it as a project, to
> work with its changes.

A folder that becomes a repository later is noticed the next time the panel opens.

### One standalone agent per folder

dux refuses a second standalone agent in a folder that already has one. Coding CLIs
remember their conversation history per directory, so the second agent would silently
pick up the first one's conversation. To put several agents on one directory, add it as a
project instead: agents there each get their own worktree, and [tabs](/docs/agent-tabs)
too.

## Choosing a provider at creation time

Every agent is tied to one provider. At creation, dux uses the default configured for
that project:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
default_provider = "claude"
```

With no project-level default, and for a standalone agent, which has no project, dux
falls back to the global default:

```toml
[defaults]
provider = "claude"
```

All three levels are editable from either front end:

- **Terminal UI:** the `change-default-provider`, `change-project-default-provider`, and
  `change-agent-provider` palette commands.
- **Browser:** the global default is the **Default provider for new agents** row in
  **Preferences…**, a project's default lives in **Project settings…** on its `⋯` menu,
  and an agent's provider in **Change agent provider…** on its own `⋯` menu. A single tab
  is retargeted with **Change provider…** on the tab's `⋯` menu.

Swapping an agent's provider never yanks a running session out from under itself. It
takes effect the next time that tab launches.

## Auto-reopening agents on startup

Agents are persistent. Quit dux and reopen it and agents can resume automatically if
`auto_reopen_agents` is on. The setting lives at two levels:

```toml
# Global default: applies to all projects unless overridden
[ui]
auto_reopen_agents = false

# Per-project override stored in config.toml
[[projects]]
id   = "a4f3..."
auto_reopen_agents = true
```

Toggle every level without editing the file. In the terminal UI,
`toggle-project-auto-reopen-agents` flips the selected project's setting and
`toggle-agent-auto-reopen` flips a single agent's. In the browser, the global switch is a
**Preferences…** row, the project one is in **Project settings…**, and an agent's own is
**Enable/Disable agent auto-reopen** on its `⋯` menu. Changes take effect the next time
dux starts.

If an agent's provider command is not found at reopen time, the worktree is left intact
and the error is reported. The agent still appears in the list, and you can reconnect it
once the CLI is available.
