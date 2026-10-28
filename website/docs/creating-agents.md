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
filesystem. Repository not on the machine yet? Skip the shell trip:
[Cloning a repository](#cloning-a-repository) clones it, adds it as a project and starts
its first agent in one go.

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

- **Linux:** `~/.config/dux/worktrees/<project-name>/<folder>/`
- **macOS:** `~/.dux/worktrees/<project-name>/<folder>/`

The folder is the agent's name flattened into one plain folder name: every character other
than letters, digits (in any language), `-` and `_` becomes `-`. An agent named
`dependabot/cargo/tray-icon-0.26.1` gets the folder `dependabot-cargo-tray-icon-0-26-1`,
while its branch keeps the real name. A name with letters outside English gets a short code
made from the full name on the end of its folder, so two names that look alike never share
one. A very long name gets a shortened folder name ending in the same kind of code, so it
fits on disk and two long names still get folders of their own. Two names that flatten to the same folder can't both
have one, so the second create is refused and the first agent's folder is left alone.
Agents created before dux flattened folder names keep the folders they already have.

Because each agent owns a real git worktree, your project's `.gitignore`, git hooks, and
local config behave exactly as they do in the main checkout.

## Naming an agent

Every creation path that makes a branch ends at a naming prompt, and there the name IS
the branch name. Any name git accepts for a branch works, so a pull request's head branch
can always name its agent, whatever it is called:

- **Refused as you type:** spaces become dashes, and the characters git never allows in a
  branch name (`~ ^ : ? * [ \` and control characters) don't go in at all.
- **Checked when you confirm:** no `..` or `@{`, it can't be `@` or `HEAD`, it can't
  start with `-`, start or end with `/`, contain `//` or end with `.`, and no part
  between slashes may start with `.` or end with `.lock`.
- **Anything else is fine:** dots, `@`, `+`, slashes and letters outside English all work.

Tick the pet-name checkbox in the naming prompt and dux generates a two-word pet name
such as `brave-morse`, for both the agent and the branch. The checkbox starts unticked;
to make pet names the default for every new agent, turn it on permanently:

```toml
[defaults]
enable_randomized_pet_name_by_default = true
```

A standalone agent has no branch, but its name follows these same rules, and an empty
name falls back to the folder's name
([details below](#naming-a-standalone-agent)).

This is the naming prompt in the terminal UI, with the pet-name checkbox ticked and the
generated name filled in:

![The terminal UI naming prompt for a new agent, with a generated pet name in the field, a ticked pet-name checkbox, and a ticked checkbox for copying uncommitted changes.](/screens/tui-name-new-agent.png)

### Renaming an agent

Rename an agent from its `⋯` menu in the browser, or with `rename-agent` in the terminal
UI. Both open the same dialog and follow the same rules.

- **The name is what the agent shows.** The field holds the custom name you gave it, empty
  when there is none, with the name it falls back to shown faintly behind it. It follows
  the same rules as a branch name, a standalone agent's included.
- **Leave it empty to go back.** An agent with a branch shows its branch name again; a
  standalone agent shows its folder's name.
- **Also rename the git branch** is a checkbox under the field, unticked by default, for
  agents that have a branch. Tick it and dux renames the branch to the new name as well.

  - The name can't be empty then, because a branch can't be renamed to nothing.
  - Once the branch carries the new name the agent simply shows its branch name, so a
    later branch rename carries the label along too.

> [!WARNING]
> Renaming the branch only renames it on this machine. When the branch has an open pull
> request, or a copy pushed to a remote, both stay on the old name, and the checkbox says
> so before you tick it. Push the renamed branch and update the pull request yourself.

A standalone agent has no branch, so its dialog has no checkbox.

## Creating a new agent from scratch

In the browser, open a project's `⋯` menu and pick **New agent…**. In the terminal UI,
run `new-agent` and pick a project from the chooser (every project is listed, including
ones with no agents yet). Either way dux checks that project's current branch, then opens
the naming prompt.

On both surfaces the project list is ordered by what you touched most recently, so a
project you just added, or one that just received an agent, sits at the top. In the
terminal UI every project chooser shares that order, the ones for
[managing projects](#managing-a-project), browsing worktrees and opening a project
terminal included.

The terminal UI's chooser lists every project, how many agents each one has, the base
branch new agents start from, and where it lives, and its footer carries the way out to a
standalone agent:

![The terminal UI project chooser for a new agent, listing two projects with their agent counts and paths, and a footer key for creating a standalone agent instead.](/screens/tui-new-agent-chooser.png)

On confirmation dux creates a worktree on a new branch, branched from the project's
**leading branch**. Where that comes from:

- **When you add the project:** if the repository was on some other branch than its
  default, the add dialog asks whether to check the default out first. Say yes and the
  default branch leads; say no and the branch it was on leads instead.
- **Later:** checking out the project's default branch makes the default lead from then
  on.
- **If dux cannot save that change,** the checkout still happens but the old leading
  branch stays in charge, and dux tells you so in an error that stays up until you close
  it. Run the checkout again once the problem is fixed.

If the name matches an existing local branch, dux asks whether to attach to that branch
instead, which is what you want when continuing work that already started.

> [!IMPORTANT]
> Attaching matters at the other end of the agent's life. dux remembers that the branch
> existed first, so the delete dialog's "also delete the branch" box starts **unticked**
> for it, with a line saying the branch predates the agent and how many of its commits
> are pushed nowhere. Tick it and the branch goes anyway; leave it and the worktree goes
> alone. The [delete section of Web agents](/docs/web-agents#kill-versus-delete) has the full rules.

### Changing the base branch

Cloned on the wrong branch, or want new agents to start from `develop`? Change the
project's base branch, the one new agents branch from. In the browser it is **Change base
branch…** in the project's `⋯` menu; in the terminal UI it is the
`change-project-base-branch` palette command, or the same row in a project's action list
(see [Managing a project](#managing-a-project)).

#### How the list works

dux fetches `origin` first, then lists every local branch and every branch that only
exists on `origin`.

- **The fetch** is tried once more if it fails and there is time left, and dux gives up
  after 15 seconds in all. When it does, the list shows origin's branches as last fetched
  and says why.
- **The current base** is marked, and the list is searchable.
- **A branch another worktree has checked out,** usually an agent's own branch, is listed
  but cannot be picked, and the row names who holds it: git will not check out one branch
  in two places.

#### Freeing a branch an agent holds

1. Delete that agent with its worktree, and untick the box that also deletes the branch.
2. For a worktree no agent holds any more, remove it from the project's worktree manager,
   again with that box unticked.
3. For a worktree whose folder is gone, run `git worktree remove` with its path in the
   project folder. It makes git forget the worktree and leaves the branch alone. (Git
   still counts such a worktree as holding its branch until then.)

The error dux shows names the way out that applies.

> [!CAUTION]
> The agent delete dialog ticks "also delete the branch" by default for a branch dux
> created, and the worktree manager ticks it by default for every branch. Leave it ticked
> and the branch you wanted as the base is deleted along with the worktree.

#### What happens when you pick one

Pick a branch and dux asks first, because the project folder switches to that branch.
Confirm and dux checks it out in the project folder (creating the local branch from
`origin`'s first when only `origin` has it), then makes it the base. If git refuses the
switch, for example because uncommitted changes in the folder would be overwritten, dux
says so in an error that stays up, and the base stays where it was.

> [!NOTE]
> Uncommitted changes that do not conflict with the new branch travel with the switch and
> stay in the project folder, the same as with a checkout in your own terminal.

### Switching and pulling safely

These rules apply to every time dux switches the project folder to a branch or pulls into
it: changing the base branch, checking out the default branch, creating an agent, an
agent's **Pull** and **Pull project**.

**A branch that only exists on a remote.** dux creates the local branch from `origin`
when `origin` has it, otherwise from the one remote that has it. Your
`checkout.defaultRemote` setting is not consulted. When several remotes have the branch
and none of them is `origin`, dux refuses the switch and asks you to create the local
branch from the remote you want first.

> [!CAUTION]
> When the branch you switch to (or pull in) tracks a path where an ignored folder or file
> stands, git deletes or overwrites it to make room: it treats ignored files as
> disposable.
>
> dux refuses the switch or pull, and changes nothing, when that folder holds, or that
> file sits inside, something it knows about:
>
> - an agent's worktree,
> - a standalone agent's folder,
> - a project's repository, or
> - a terminal working there.
>
> Any other ignored folder or file git still replaces, as it would in your own terminal,
> so move it aside first if it matters.
>
> The same goes when the incoming commit deletes every file in a folder (git removes the
> folder too) or deletes or replaces a link, even one the branch tracks: dux refuses when
> one of those lives or works there.

> [!NOTE]
> dux's own pulls and branch switches never update submodules, even when your git config
> sets `submodule.recurse`: the submodule pointer moves, the submodule's files stay where
> they are. Run `git submodule update` yourself when you want them to follow.

### Pulling before create

By default dux pulls the leading branch first, so the new agent starts from the freshest
upstream commit:

```toml
[defaults]
pull_before_creating_agent_by_default = true
```

The pull is best-effort. It is fast-forward only, never a merge or rebase, it is skipped
for repos with no `origin` remote, and a failed pull does not block creation: the agent
starts from the local branch state and the status message says so. That includes a pull
dux refused because it would have deleted a folder it knows about (see [Switching and pulling safely](#switching-and-pulling-safely)).

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
nothing was copied.

Never copied: files matched by `.gitignore`, submodule and embedded-repository contents,
and empty directories.

## Cloning a repository

The repository you want to work on lives on a remote and not on this machine? One action
clones it, adds it as a project and starts its first agent there. In the browser it is
**Clone a repository…**, beside **Add project…** under **Projects** in the menu the
launcher's **▾** opens, and in the app menu's **Add project** submenu. In the terminal UI it is the
`clone-project` palette command.

Both open the same short form:

- **The address.** Anything `git clone` takes: `https://…`, `ssh://…`,
  `git@host:owner/repo.git`, a `file://` address, or a plain path.
- **The destination folder.** It fills itself in as your project browser's start
  directory plus the repository's name, and keeps following the address until you edit
  it. A leading `~` works; otherwise give a full path.
- **The first agent's name**, with the same pet-name checkbox as every other naming
  prompt. Leave the name empty and dux picks a pet name for you.

Then dux runs a full clone, checks out the remote's default branch, adds the folder as a
project with that branch as its base branch, and starts the agent on a fresh branch under
the provider every new project gets by default. One progress message covers the lot and
ends with one outcome: the agent is running, or what stopped it and how far it got.

- **In the browser** the dialog closes as soon as the clone starts, and dux selects the
  new agent when it appears, however long that takes.
- **In the terminal UI** the status line carries the progress and the outcome.
- **A problem dux spots straight away** keeps the form open with the reason. In the
  terminal UI, a refusal or a failed clone that arrives after the form has closed hands
  your typed values back the next time you open it, so a typo in the address costs you one
  fix rather than the whole form.

> [!IMPORTANT]
> **dux never asks for anything while it clones.** It uses your own git setup, and nothing
> that would ask is ever waited on, a password pop-up included: it fails straight away with
> git's or ssh's own message instead.
>
> - **Works as in your shell:** your ssh agent and keys, URL rewrites, and credential
>   helpers that answer without asking (a stored token, a keychain).
> - **Do first, once:** connect to a host you have never used from a shell (or clone
>   anything from it) so it is trusted, and load an ssh key with a passphrase into your
>   agent.

A few more things worth knowing:

- **The machine that clones is the one dux runs on.** A path or `file://` address is read
  from that machine's disk, not from the device your browser is on, and the destination
  is on that machine too.
- **The destination must be new or an empty folder**, its parent folder must already
  exist, and it cannot already be a project. dux will not clone into a symbolic link, and
  will not start a second clone into a folder it is already cloning into.
- **A user name and token in the address work, and dux keeps them out of sight.** Every
  message and log line dux writes shows the address without them. git itself still saves
  the address as you typed it in the clone's own `.git/config`, which is your call.
- **Submodules are not initialized**, the same as everywhere else in dux.
- **The `dux` command line has no clone command.** Clone from the browser or the terminal
  UI.

### How long a clone runs

There is no time limit, only a silence limit. A big repository is slow, not stuck.

- **Quiet for too long:** a clone that goes quiet for
  [`[git] clone_stall_seconds`](/docs/configuration#how-long-a-clone-may-go-quiet-git)
  (5 minutes by default) is stopped, and the message says so.
- **Still going after 30 minutes:** its spinner turns into a warning while git carries on,
  and the outcome still arrives when it ends.
- **No cancel button:** quitting dux stops a running clone.

### When a clone stops partway

dux never deletes anything you put there. The one folder it ever removes is a destination
it created itself for this clone, and only while that folder is still empty. Everything
else stays, so what is left depends on where it stopped. The message always says which of
these happened and what to do next.

| What stopped it | What you have afterwards |
|---|---|
| A check before git ran (bad name, folder not empty, and so on) | Nothing changed. |
| git failed (wrong address, no access, the host refused) | Nothing added. git clears out what it wrote. |
| The clone went quiet for too long, or dux quit mid-clone | Nothing added. The destination may hold part of a clone, and dux leaves it there (see below). |
| The remote has no commits yet | The clone stays, no project is added. |
| The remote's default branch could not be checked out | The clone stays, no project is added. |
| Adding the project failed | The clone stays, no agent is created, and the message gives the reason. |
| A branch named like the agent already exists in the clone | The project is added, no agent is created. Use New agent with another name. |
| The agent could not start | The project is added, and the message says why the agent is missing. |

What to do next, for the cases that need more than the table says:

- **git failed:** if dux created the destination folder, it goes too. An empty folder you
  made yourself stays.
- **Quiet too long, or dux quit:** remove the leftover destination before trying again.
  dux refuses a folder that is not empty, and says an earlier clone may have been
  interrupted there.
- **No commits yet:** use Add project on the folder, which offers to make a first commit.
- **Default branch not checked out:** check a branch out in the folder, then use Add
  project.
- **A branch named like the agent exists** (the default branch included): pick another name
  with New agent, which also offers to attach to that branch.

## Creating an agent from a GitHub PR

In the browser, **New agent from PR…** in the menu the **▾** beside **New agent…** at the
bottom of the sidebar opens, under **Agents**, or in a project's own `⋯` menu to start from that
project. In
the terminal UI, the `new-agent-from-pr` palette command.

This path needs the `gh` CLI installed, authenticated with `gh auth login`, and
`github_integration` on, which it is by default:

```toml
[ui]
github_integration = true
```

dux checks `gh` at startup and again whenever you switch the integration on, so running
`gh auth login` while dux is up is enough. If `gh` is missing or none of its logins work,
the path is hidden outright on both front ends. One expired login does not take the
others down: if you are signed in to two hosts and one token is stale, the working host
keeps the GitHub features on.

**A failed check is never final.** While GitHub features are unavailable, dux quietly
asks `gh` again every few minutes and turns them back on the moment it works, so a
GitHub rate limit or a short outage while dux was starting does not mean restarting it.
The interval is yours:

```toml
[ui]
github_probe_interval_secs = 300  # 0 turns the periodic re-check off
```

`0` disables the periodic re-check entirely; asking on demand still works. Any other
value is clamped to between 30 seconds and 21600 seconds (6 hours), and dux logs a
warning once when it clamps one, so a mistyped `1` cannot have it launching `gh` every
second.

You can also ask right away: **Re-check GitHub** in the browser's settings menu under
**Configuration** (shown while GitHub integration is on), or the `recheck-github` palette
command in the terminal UI. Either way dux tells you what it found, including why it
still cannot use `gh`.

**GitHub Enterprise works, on any hostname `gh` is logged in to.** A company server at
`git.company.example` is treated exactly like `github.com` once
`gh auth login --hostname git.company.example` succeeds, for the PR banner and for this
path.

An older `gh` that cannot report its hosts gets two stricter fallbacks:

- **A single yes-or-no login check:** any host in trouble switches the GitHub features
  off.
- **Host recognition:** `github.com` and `github.*` only. If your enterprise host is
  spelled anything else, upgrade `gh`.

### The reference comes first, and dux works out the project

Open this from the global command and the first thing you see is the reference field. No
project is asked for: paste the link and dux compares the repository it names against
every project you have.

- **One project is a checkout of that repository.** dux goes straight on to resolve the
  pull request and name the agent.
- **Two or more are.** dux shows you just those and asks which one this agent belongs in.
- **None is.** When the reference names a repository and a pull request number on a host
  `gh` is signed in to, dux looks the pull request up and offers to set it up:

  - The terminal UI and the browser show the pull request's title and branch and ask
    before they clone the repository, or, when the folder it would clone into already
    holds a checkout of it, before adding that folder as a project.
  - The agent then starts on the pull request's code. The folder and the agent's name are
    editable, and **Choose an existing project** keeps the project picker one stop away.
  - When dux can tell beforehand that the agent would need a
    [review copy](#naming-and-fetching), the offer says so and asks first: the same
    warning and unticked **Create a review copy** box as below, and dux won't clone or add
    until you tick it. A name you type that is spelled like a review copy of the pull
    request's branch gets the same question.
  - A reference with no number, or a pull request dux could not look up, gets the project
    picker instead, with the lookup's own sentence when that is what failed.

> [!IMPORTANT]
> The clone is the same one [Cloning a repository](#cloning-a-repository) does. It uses
> `gh`'s git protocol setting for that host and never prompts for anything. Over SSH,
> connect once in a terminal first with `ssh -T git@<host>`, so the host is known; over
> HTTPS, a private repository needs git credentials set up outside dux, for example with
> `gh auth setup-git`.

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
browser the reference and the name are two fields in one dialog.

The name you confirm is
what the fetch targets: dux fetches the PR's head ref into that local branch, then
creates a worktree on it. If dux cannot fetch the pull request, nothing is created and
the message says why.

#### If a local branch of that name already exists

From a previous fetch, say. dux first fetches the pull request's head and checks that your
branch contains it.

- **It contains the pull request:** dux attaches to it, and the delete dialog later offers
  that branch unticked, the way it does for any branch you had first.
- **It is simply behind,** with no commits of its own and not checked out anywhere: dux
  moves it forward to the pull request's latest commit and attaches to it, and the
  confirmation says it did.
- **It has gone its own way:** dux leaves your branch alone and offers a
  [review copy](#review-copies) instead.

With no local branch of that name, dux fetches the pull request head into a new local
branch, even when your project already has a branch of that name on the remote, because a
pull request from a fork often shares its branch name with an unrelated branch there.
That local branch is dux's own, and the delete box is ticked by default.

Nothing dux does
to it reaches the remote: the branch on GitHub, and the pull request itself, are
untouched either way.

#### If another agent already has the code

dux creates nothing, whatever name you typed. An agent has it when its branch is the pull
request's own branch (or a review copy of it) and contains the pull request's latest
commit, or when its branch sits exactly on that commit. An agent working on a pull request
stacked on top of this one does not count.

dux tells you which agent has it and where, and suggests opening a new tab in that agent,
which works in the same folder on the same code. An agent on that branch that does not
have the pull request's code yet gets the fresh copy below.

#### Review copies

If the pull request's branch is already checked out somewhere (by your project folder, or
by a worktree in the middle of a rebase or a bisect), git will not check it out a second
time. A branch that has gone its own way from the pull request is not the pull request's
code either. In both cases dux can make a **review copy** instead:

- **What it is:** the pull request fetched into a new branch named after it, in a clean
  worktree. That is handy when you want a second agent to review the work with none of the
  first one's leftovers.
- **Its name:** `feat-x-review` for a branch called `feat-x`, then `feat-x-review-2` and so
  on, up to `-review-20`.
- **Its content:** the pull request as GitHub shows it. The message that confirms it says
  where the original branch is checked out.
- **Its link:** the new agent is linked to the pull request, so its status pill follows
  it. If the link cannot be made, the confirmation says so as a warning, and you can attach
  the pull request to the agent yourself.
- **Its pushes:** a push from it goes to its own branch, never to the pull request's.

dux never makes the copy on its own. It stops first, creates nothing, and tells you why
the branch cannot be used and what name the copy would get. Because a push from the new
agent lands on that copy's branch and not on the pull request, you have to agree to that:

- **In the browser,** the New agent dialog reopens with your reference and name still
  filled in, a warning, and an unticked **Create a review copy** box. **Create** stays
  disabled until you tick it.
- **In the terminal UI,** the name prompt reopens the same way.
- **On the command line,** the same refusal is printed, and running the command again with
  `--review-copy` agrees to it.

If the branch changes between the warning and your answer so that the copy would be needed
for a different reason or under a different name, dux asks again instead of going ahead.

> [!IMPORTANT]
> A review copy is a branch of its own. Pushing from the agent never updates the pull
> request, so if you meant to add commits to the pull request, free its branch and create
> the agent again instead.

> [!WARNING]
> Commits you have not pushed yet are not in the copy.
>
> - **The copy is simply behind the busy branch:** the confirmation leads with how many
>   commits it lacks, as a warning. Push first if the reviewer should see them.
> - **The two have gone separate ways** (a pull request that was rebased or force-pushed,
>   or one from a fork whose branch happens to share a name with yours, such as `main`):
>   there is no count and no warning, because the copy is not an older version of the busy
>   branch and nothing in it is simply missing.

#### Names you type yourself

- **Spelled like a review copy** of the pull request's branch, such as `feat-x-review`:
  that makes a review copy all the same, so dux asks about it the same way.
- **Any other new name** needs no question.
- **Checked out elsewhere, and not the pull request's branch:** refused, with the place it
  is checked out, so you can pick another.
- **Already a local branch without the pull request's code:** refused too.

### How PR status stays fresh

With `github_integration` on, dux shows a PR status pill on each agent branch. These
refresh it right away:

- bringing the agent to the foreground,
- creating the agent,
- renaming its branch, and
- the agent exiting.

A background poll notices the rest, a push included, along with changes made on GitHub
itself:

```toml
[ui]
# Seconds between PR-status polls. This is what notices a push or a change made
# on GitHub. Set to 0 to rely on the events above alone.
pr_poll_interval_seconds = 180

# Seconds between those polls for agents under "Inactive" in the agent list:
# the detached ones, and the ones whose process exited. Nobody is working in
# one, so its pull request is polled on this much slower clock. The default is
# 12 hours. Set to 0 to stop polling inactive agents entirely.
pr_poll_inactive_interval_seconds = 43200
```

How the two settings behave:

- **Ranges:** any non-zero `pr_poll_interval_seconds` is held to 30 to 21600 seconds, and
  any non-zero `pr_poll_inactive_interval_seconds` to 30 to 604800 seconds (7 days).
- **Inactive rounding:** the inactive period rounds up to the next active poll, and a value
  below `pr_poll_interval_seconds` behaves as that interval.
- **Turning the active poll off** (`0`) turns the inactive poll off as well.
- **Coming back from Inactive,** because you reconnected to it or started it again, checks
  the agent right away rather than waiting out the slow clock. The event-driven refreshes
  above ignore which section an agent is in.

When a branch name is reused, dux follows the most recent pull request on it, preferring
one that is open.

A pull request that was merged, and its branch deleted, before dux ever saw it is still
found by the branch's name, and so is a newer one that replaced a pull request dux already
knew. Because branch names get reused, dux takes such a match only when:

- the pull request came from the agent's own repository rather than a fork,
- its last commit is in the agent's local branch, and
- for a branch dux created, it was opened after the agent was.

Anything else is left alone, and you can attach the pull request by hand.

If your GitHub API quota runs low, or GitHub starts erroring, dux pauses PR checks until
it recovers and tells you: a status line in the terminal UI, a toast in the browser.

#### The PR banner

Above or below the agent's terminal, a one-line banner carries the pull request's number,
its state and its title, on both surfaces.

- **Click it:** clicking the banner opens that pull request in your browser, wherever you
  click it.
- **From the keyboard:** in the terminal UI the `open-current-pr` command does the same. It
  is the way in while an agent pane is maximized, since the maximized surface covers the
  banner (the banner also steps aside while you are typing into the pane).
- **Which side:** `pr_banner_position` under `[ui]` is `"top"` or `"bottom"`, and the
  default is `"bottom"`. Anything else falls back to the default.

```toml
[ui]
pr_banner_position = "bottom"
```

The terminal UI's `toggle-pr-banner-position` palette command and the web's Preferences
flip it too.

## Creating an agent from an existing worktree

Either surface opens a picker of every git worktree for the project's repository:

- **In the browser:** **Worktrees…** in a project's `⋯` menu, or **New agent from existing
  worktree…** in the app menu, which asks for the project first and offers **Back** to
  return to that list.
- **In the terminal UI:** the `new-agent-from-worktree` palette command.

The picker has two groups:

- **Managed worktrees**, already under dux's `worktrees/` directory. One with no agent
  yet gets a new session attached without touching the branch or files. An adopted
  worktree's branch came with it, so the delete dialog offers that branch unticked, with
  a line saying it came with the worktree.
- **External worktrees** (terminal UI only), which exist in the repository but live
  outside dux's managed directory, such as one you created with `git worktree add`. dux
  forks these: a new managed worktree branched from the external worktree's current
  `HEAD`, with dirty and untracked files copied across. Gitignored files do not travel.

The main checkout is never selectable; dux keeps that for you. Worktrees that already
have an agent are shown but disabled, with a tooltip explaining why (in the browser they
sit under **Already has an agent**); in the terminal UI, selecting one reports "That
worktree already has an agent."

### Deleting a worktree, and its branch

The browser's **Worktrees** dialog doubles as a manager: an unused worktree's `⋯` menu
offers **Delete worktree…**. The terminal UI has the same manager as the
`manage-worktrees` palette command. The project picker in front of it labels each project
with how many worktrees it has.

This is the terminal UI's manager: free worktrees on top, the ones an agent or something
else is holding below them, each row naming its branch and saying whether there is
uncommitted work in it.

![The terminal UI worktree manager listing one removable worktree and two held by an agent, each with its branch and an uncommitted-changes note.](/screens/tui-worktree-manager.png)

> [!CAUTION]
> Deleting a worktree removes the directory from disk. The confirmation names the branch
> and the full path, and says specifically when there are uncommitted changes to lose. It
> also offers to delete the branch, **ticked by default**; untick it and the branch
> survives. If git refuses the deletion, dux reports the branch as still there with git's
> own reason.

![The terminal UI confirmation for deleting a worktree, naming the path, warning about the uncommitted changes, and offering a ticked checkbox that also deletes the branch.](/screens/tui-worktree-delete-confirm.png)

Worktrees that cannot be removed are listed but unselectable, because removing one from
under a session leaves it broken. The groups differ a little per surface.

In the terminal UI:

- **Held By An Agent:** a worktree an agent holds. Delete the agent instead. The row reads
  **held by an agent**, and adds `in use:` and what is using it only when something
  besides that agent is using the folder as well, such as a terminal open in it.
- **Also in that group:** a worktree something else is still using (an agent being created
  in it, a terminal open in it, the CLI of an agent you just deleted still stopping, a
  standalone agent working inside it). Its row says `in use:` and what is using it.
- **Being Removed:** a worktree whose agent was just deleted, until it is gone. It cannot
  be removed a second time.
- **Left Over After A Delete:** a folder git let go of when its worktree was deleted, but
  that could not be deleted because something kept writing into it. The row names the
  branches kept because of it. Pick it for **Copy path** and **Delete folder…**.

In the browser the same cases are three groups: **Already has an agent**, **In use** (each
row says what is using it) and **Being removed**, plus **Left over after a delete**.

> [!IMPORTANT]
> When a delete leaves its folder behind, dux keeps the branches it was going to delete,
> because the folder still holds their files. **Delete folder…** confirms first and offers
> the kept branches too; a branch that has moved since is kept. The rules are in
> [When something keeps writing into the worktree](/docs/web-agents#when-something-keeps-writing-into-the-worktree).

Either manager is how you remove a branch belonging to a worktree that has no agent. For a
worktree that does have one, the agent's own delete dialog is the place: it names the
branch, says whether it predates the agent, and offers to remove it there and then. Once
the worktree is gone neither surface can reach the branch, and `git branch -D` is the way.

## Managing a project

Everything you can do to a project lives in one list per project. In the terminal UI, run
`manage-projects`: it lists every project with its agent count, its base branch and its
folder (a missing folder gets a warning sign), plus any **orphaned group**, agents whose
project record is gone, shown by a short id. Pick one and its actions open, headed by the
project's name, its folder, its base branch and the branch the folder is on right now:

- **New agent…**, **New agent from PR…** (only with GitHub integration on),
  **Worktrees…** and **New terminal at the project root**
- **Pull project**, **Check out default branch…** and **Change base branch…**
- **Project info…**, a read-only page of the project's settings and counts
- **Default provider…**, the auto-reopen toggle, **Startup command…** and
  **Environment…**
- **Startup command logs for all agents…**
- **Delete project…** and **Remove project…**

**Delete project…** and **Remove project…** differ in what happens to the worktrees:

- **Delete project…** removes the project and every agent's record at once, then each
  agent's worktree in the background, the same way deleting one agent with its worktree
  does: each one waits for its agent to stop and for anything still running in it. One
  status follows the whole delete and, when it ends, names any worktree that could not be
  removed and why.
- **Remove project…** forgets the project and its agents and leaves every worktree on
  disk.

If an agent is still being created in the project when you confirm a delete, it waits for
the creation to finish (up to the top-level `removal_wait_seconds`, 120 seconds by
default) and takes the new agent along. No new agent can be created in a project while it
is being deleted.

Moving around the list:

- **Coming back:** anything you open from the list comes back to it when it closes,
  whether you cancel it, close it or save it: the questions, Project info, the settings
  editors, the worktree manager and the startup command logs.
- **Escape** on the list goes back to the project list, and a second Escape closes it.
- **An orphaned group** offers **Remove project…** and nothing else, which clears those
  agents' records and leaves their worktrees on disk.
- **A missing project folder:** the rows that need it (a new agent, the project terminal
  and changing the base branch) are shown as unavailable and say why.

The browser has the same actions in each project's `⋯` menu.

Every project palette command still works on its own. With an agent selected it acts on
that agent's project; with no agent selected it opens the project list first and acts on
the one you pick. `copy-path` does the same: the selected agent's folder, or the picked
project's.

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

Every way in opens the same folder browser. Any folder is accepted: it does not have to
be a git repository, and dux initializes nothing in it.

- **In the browser:** **New standalone agent…** from the launcher's **▾** menu or
  the app menu's New submenu, and **Add standalone agent…** at the bottom of the New agent
  project list, for when you went looking for a project and none fits.
- **In the terminal UI:** a key anywhere in the agents pane, a key inside the "New agent in
  project" chooser too (so you can change your mind once you are already there and no
  project fits), and the `new-standalone-agent` palette command. The `?` help overlay names
  the keys, and you can rebind them under `[keys]` in `config.toml`.

> [!NOTE]
> Once you start filtering inside the chooser, the key that still works is a modifier
> chord, because plain letters go into the search box. GNU Screen's flow control can
> swallow that chord before dux ever sees it; the palette command still works there, and
> so does rebinding the key to something your terminal passes through.

### Naming a standalone agent

Both surfaces then ask what to call the agent. The name is optional, and backing out of
that question creates nothing and leaves the folder untouched.

- **Empty:** the agent is named after the folder, turned into a valid agent name (`My
  Notes! (2026)` becomes `My-Notes-2026`). A folder with nothing usable in its name gives
  `standalone-agent`. The same happens when you clear the name later when renaming.
- **Typed:** it follows the same rules as any agent's name, even though no branch is
  created: any name git accepts for a branch, with spaces becoming dashes as you type.
  See [Naming an agent](#naming-an-agent).

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
> remove it, ever.
>
> - Deleting the agent removes dux's own record and nothing else. The delete dialog says
>   so: there is no "also remove the worktree" checkbox, because there is no worktree.
> - A factory reset skips the folder too.

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

A quiet panel keeps its header and its `⋯` menu, so you can still hide it or open the
folder in the editor. Its git items stay in that menu greyed out with the reason, because
they are the folder's own and come back once it is a repository. Pushing and pulling are
not among them: they stay absent, as above.

![The Changes pane of a standalone agent whose folder has no git repository, its menu open: Commit and Refresh changes greyed out under a line saying the folder has no git repository, and Hide Changes pane still available.](/screens/changes-quiet-menu.png)

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
