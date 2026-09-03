---
title: Agents from the browser
description: Create, fork, adopt, and manage agents in server mode, including starting from a pull request with no project chosen, plus provider tabs, dormant tabs after a restart, notifications, and kill versus delete.
group: Web UI
order: 64
---

The whole agent lifecycle runs from the browser: spin one up (in a worktree and branch dux
manages, or standalone in a folder you already have), fork it, adopt an orphaned worktree,
retarget its provider, and tear it down. This page is the click-by-click version;
the concepts behind projects, agents, and providers are in
[Creating agents](/docs/creating-agents).

## Adding a project

> [!IMPORTANT]
> The **Add project** dialog browses the **server's** own disk, not your laptop's, starting
> from its configured start directory, normally the server's home directory.

Repositories carry a small "git" badge. Pick one, give it an optional name, and dux may
show a pre-flight step first:

- If the repo is checked out to something other than its default branch, it offers to check
  that branch out before adding.
- If the repo has no commits yet (a fresh `git init`), it offers to make the initial commit
  for you.

The confirm button's label adapts to whichever applies. A project's `⋯` menu carries
project settings, project info, and remove project, alongside the agent-creation actions
below.

## Creating an agent

Two doors, differing in what you start from.

**The launcher** sits at the bottom of the sidebar (and on the mobile hub): one filled
button and one `⋯`. The button is a single click to a plain new agent, reading **Add
project** instead while you have none. The `⋯` groups everything else you can create from
nothing, under **Agents** (from a pull request, from an existing worktree, or standalone in
a folder you already have), **Terminals** (a standalone shell) and **Projects**. Use this
door when you have not picked a project yet; it is the only place the reference-first flow
lives. The Agents header above the list carries a **+** too.

### Starting from a pull request, with no project

Pick **New agent from PR…** from that `⋯` segment and the dialog opens with the reference
field first. **No project is chosen and none is asked for.** Paste a PR link (or
`owner/repo#123`, or a bare `owner/repo`) and dux compares the repository it names against
every project you have:

![The New agent from PR dialog with a pull request reference typed into its first field.](/screens/new-agent-from-pr-dialog.png)

- **One project is a checkout of that repository** and dux goes straight on to resolve the
  pull request.
- **Two or more are** and dux shows you just those and asks which.
- **None is** and dux names the repository it could not place, then offers the project
  picker.

> [!IMPORTANT]
> **dux will not clone a repository it does not have.** Point it at a checkout that already
> exists on the server, or clone one yourself first.

A number on its own (`#123` or `123`) is the one form this door cannot take, since it does
not say which repository it belongs to. dux refuses it and points you at "choose an existing
project" under the field, which switches to the project-first mode below and brings anything
you typed with it.

The full list of accepted spellings, and what happens to projects dux cannot compare, is in
[Creating agents](/docs/creating-agents).

### Starting from a project

The **New agent** dialog, reached from a project's `⋯` menu, skips the resolution step: the
project is already chosen, so **New agent from PR…** here takes a reference scoped to that
project and a bare `123` is meaningful.

![The New agent dialog with a branch name typed, ready to create the worktree and launch the agent.](/screens/new-agent-dialog.png)

- **New agent…** creates a fresh git worktree and branch and launches the agent. The branch
  name is optional; leave it blank and dux generates a memorable pet name.
- **New agent from PR…** fetches a pull request's head branch into a new worktree. Give it
  a PR URL, `#123`, or just `123`. It appears only when GitHub integration and the `gh` CLI
  are available.
- **Worktrees…** opens that project's worktree manager, below.

**Fork agent…**, in an existing agent's `⋯` menu, opens the New agent dialog locked to fork
mode and copies that agent's current files into a new worktree and branch.

Whichever way you create an agent, it launches immediately. The name input sanitizes itself
to valid branch characters as you type.

### The worktree manager

**Worktrees…** lists every worktree dux manages for the project, with its branch, its path
on disk, and a warning when it is holding uncommitted changes. Pick one that has no agent
and it becomes a live agent again on its existing branch, which is how you reclaim work a
deleted agent left behind.

![The Worktrees dialog listing every worktree dux manages for a project, with the agent holding each one.](/screens/worktree-manager-dialog.png)

Each unused worktree carries a `⋯` menu with **Delete worktree…**, which removes the
directory from disk after a confirmation naming the branch and the full path. That
confirmation carries an **"Also delete the branch"** checkbox, ticked by default, because a
branch left behind makes creating an agent under that name later fail with "branch already
exists". Untick it and dux removes the working directory only. A worktree that is not on a
branch has no branch to offer, so the checkbox does not appear. If git refuses the branch
deletion (it is checked out somewhere else, say), the worktree still goes and dux tells you
the branch survived and quotes git's reason.

A worktree that already has an agent names that agent and offers no delete. Delete the agent
instead.

> [!TIP]
> This is where you remove the branch of a worktree that has no agent. A worktree that does
> have one is not listed as removable, and its agent's own delete dialog is the place: it
> offers the same branch box. Once a worktree is gone neither surface can reach its branch,
> and `git branch -D` is the way.

The terminal UI has the same manager, as the `manage-worktrees` palette command, and both
drive the same rules.

## Managing an agent

An agent's `⋯` menu is where the rest lives: rename it (a display title, the branch keeps
its own name), change its provider, view its info, inspect its project's environment and
[startup command](/docs/startup-commands), and read startup-command logs.

![An agent's menu open beside its sidebar row, listing rename, fork, change provider, editor and delete actions.](/screens/agent-session-menu.png)

**Change provider** and **Force recreate** are the two knobs around resume:

- **Change provider** swaps which CLI the agent runs, taking effect the next time that tab
  launches rather than yanking a running session. That launch follows the same per-provider
  resume rule as any other: it resumes the new provider's prior conversation on the worktree
  only if it supports resume and no other live tab of the agent is already running that same
  provider. Copilot never resumes, so a tab switched to Copilot always starts fresh.
- **Force recreate** is the explicit "start clean, abandon the conversation" button.

## Provider tabs

An agent can run more than one provider session in its single shared worktree, as **tabs**.
With two or more, a strip of pills appears atop the terminal, each a live provider session
against the same files. Add one with the split **+** button (the main button uses the
project's default provider, the caret lets you pick), switch with a click, retarget or close
from a pill's `⋯` menu. There is no privileged "main" tab: closing the agent's first tab
hands its place to the next tab in the strip, and only an agent's very last tab refuses to
close (detach the agent instead).

Resume is automatic and decided per provider, never a toggle. A launching tab resumes its
provider's prior conversation when it is the only live tab of that provider, and starts fresh
otherwise, so a Claude tab and an OpenCode tab can both resume side by side while two Claude
tabs would not collide. Copilot is excluded from resume entirely. The full model, including
closing a tab and the per-agent tab cap, is in [Agent tabs](/docs/agent-tabs).

### Dormant tabs after a restart

When the server restarts, every one of an agent's tabs comes back **dormant**, with no
process behind it. An **extra** tab shows a card saying it is not running, with a **Start
session** button, and nothing launches until you ask. The agent's **first** tab is the
exception: selecting the agent starts it, because asking for the agent is asking for that
tab.

A tab whose last run ended badly (it failed to launch, or the provider exited with an
error) waits for you too, first tab included: it shows the same card, and only **Start
session** retries it. That is deliberate, so a tab that cannot come up does not relaunch
every time you open the agent.

Starting a dormant tab picks up that provider's most recent conversation in the worktree,
unless another tab of the same provider is already running or the provider cannot resume, in
which case it starts fresh. See [how resume works](/docs/agent-tabs#how-resume-works). To
reach an older conversation, use the provider's own history command.

## Attention and notifications

When an agent needs you (a permission prompt, a finished turn), its sidebar icon turns cyan
and pulses, its tab-strip pill gains a small cyan dot, the browser tab title gains a count
like `(2) dux`, and the favicon grows a small cyan dot. The flag clears the moment you look
at that agent. The whole model, and how to make sure your agents emit the signal, is in
[Attention indicators](/docs/attention-indicators).

Server mode can also raise a **real browser desktop notification** when an agent asks for
you. It is strictly opt-in: dux never auto-prompts. Open **Preferences…** from the cog menu,
use **Enable browser notifications** once, and grant permission. It fires only while the dux
browser window is hidden or unfocused, and only for the agent whose view you have open. This
is governed by the `web_notifications` capability, detailed in
[Terminal capabilities](/docs/terminal-capabilities).

> [!WARNING]
> Browsers only allow the notification-permission prompt on secure origins, meaning HTTPS or
> `localhost`. Over a plain-HTTP Tailscale or LAN URL, **Enable browser notifications** can
> be silently blocked before it ever shows a prompt. Loopback access or a TLS-terminating
> proxy gets you a working prompt.

## Kill versus delete

**Kill (detach)** stops the running session but keeps the agent and its worktree in a
detached, reopenable state. The **Task Manager**, in the app menu behind the cog button,
lists every running agent tab and companion terminal with its CPU, memory, and process count,
expands any of them to show the child processes underneath, and stops them one by one or all
at once. Every stop confirms first.

> [!WARNING]
> Companion terminals, unlike agents, are **destroyed** when killed, not detached.

**Delete** removes the agent from dux entirely. It is the one destructive per-agent action
dux tints red, and it always confirms first. The confirmation includes an unchecked "also
delete the git worktree" box, so by default your worktree survives a delete.

Tick it and a second box appears, named after the branch. It only appears there because git
will not delete a branch that is still checked out in a worktree, so with the worktree
staying there is nothing to offer. Where it starts depends on where the branch came from:

- **Ticked**, for a branch dux created for the agent. That is the old behavior. Untick it to
  keep the branch.
- **Unticked**, for a branch that already existed, or that was adopted along with an existing
  worktree. Underneath it dux says which of the two it is, and adds "It has N commits not
  pushed anywhere" when there are any. In a repository with no remotes it says so differently,
  because there the count is the branch's whole history and nothing was ever going anywhere.
  Tick it and the branch goes anyway; this is the only way to remove such a branch from dux
  once its worktree is gone.

If the worktree has moved onto another branch since the agent was created, the tick removes
**both**: the branch it is on now and the one it was born on, which is what keeps creating
an agent under the old name from failing with "branch already exists". The box names both
branches, dux says which one predates the agent, and the commit count covers the two of them
together.

If git refuses to delete a branch dux did try to remove, dux says which branch is still there
and why rather than reporting a deletion that did not happen.

> [!IMPORTANT]
> Worktrees are your data. Deleting an agent leaves its worktree on disk unless you explicitly
> opt in, which is why "adopt an existing worktree" above can bring one back.
