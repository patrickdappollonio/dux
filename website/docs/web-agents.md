---
title: Agents from the browser
description: Create, fork, adopt, and manage agents in server mode, including starting from a pull request with no project chosen, plus provider tabs, dormant tabs after a restart, attention indicators and browser notifications, and the difference between killing and deleting an agent.
group: Web UI
order: 64
---

The whole agent lifecycle runs from the browser: spin one up, fork it, adopt an
orphaned worktree, retarget its provider, and tear it down. Every agent is a real
git worktree on its own branch, exactly as it is anywhere else in dux, so a browser
where you have no terminal at all is still a full workspace and not a dashboard over
one. This page is the click-by-click version; the concepts behind projects, agents,
and providers are laid out in [Creating agents](/docs/creating-agents).

## Adding a project

Before there is anything to create an agent in, there has to be a project. The
**Add project** dialog browses the **server's** own disk (not your laptop's),
starting from its configured start directory, normally the server's home
directory. Folders that are git repositories carry a small "git" badge so you can
tell them apart from plain directories at a glance. Pick one, give it an optional
name, and dux may show a pre-flight step first: if the repo is checked out to
something other than its default branch it offers to check that branch out before
adding, and if the repo has no commits yet (a fresh `git init`) it offers to make
the initial commit for you. The confirm button's label adapts to whichever of
these applies. Once a project exists, its `⋯` menu carries project settings,
project info, and remove project, among the agent-creation actions below.

## Creating an agent

There are two doors into agent creation, and they differ in what you start from.

The **New agent** split button sits at the bottom of the sidebar (and on the
mobile hub). Its one-click primary opens the picker for a plain new agent; the
attached `⋯` segment carries the same three variants below. This is the door to
use when you have not picked a project yet, and it is the only place the
reference-first flow lives.

### Starting from a pull request, with no project

Pick **New agent from PR…** from that `⋯` segment and the dialog opens with the
reference field first. **No project is chosen and none is asked for.** Paste a PR
link (or `owner/repo#123`, or a bare `owner/repo`) and dux compares the repository
it names against every project you have, then takes you to the right one:

- **One project is a checkout of that repository** and dux goes straight on to
  resolve the pull request.
- **Two or more are** and dux shows you just those and asks which.
- **None is** and dux names the repository it could not place, then offers the
  project picker. **dux will not clone a repository it does not have**: point it
  at a checkout that already exists on the server, or clone one yourself first.

A number on its own (`#123` or `123`) is the one form this door cannot take,
because by itself it does not say which repository it belongs to. dux refuses it
and points you at the secondary action under the field, "choose an existing
project", which opens the project selector and puts the dialog into the
project-first mode below. Anything you have already typed comes with you.

The full list of accepted spellings, and what happens to projects dux cannot
compare, is in [Creating agents](/docs/creating-agents).

### Starting from a project

The **New agent** dialog offers three ways in, reached from a project's `⋯` menu.
Coming in this way skips the resolution step entirely, exactly as it always did:
the project is already chosen, so **New agent from PR…** here takes a reference
scoped to that project and a bare `123` is perfectly meaningful.

- **New agent…** creates a fresh git worktree and branch and launches the agent.
  The branch name is optional; leave it blank and dux generates a memorable pet
  name.
- **New agent from PR…** fetches a pull request's head branch into a new
  worktree. Give it a PR URL, `#123`, or just `123`. This one appears only when
  GitHub integration and the `gh` CLI are available.
- **Worktrees…** opens that project's worktree manager. It lists every worktree
  dux manages for the project, with its branch, its path on disk, and a warning
  when it is holding uncommitted changes. Pick one that has no agent and it
  becomes a live agent again on its existing branch, which is handy after a
  restart or for reclaiming work a deleted agent left behind. Each unused
  worktree also carries a `⋯` menu with **Delete worktree…**, which removes the
  directory from disk after a confirmation naming the branch and the full path.
  That confirmation carries an **"Also delete the branch"** checkbox, ticked by
  default, because a branch left behind is what makes creating an agent under
  that name later fail with "branch already exists". Untick it and dux removes
  the working directory and nothing else. A worktree that is not on a branch
  has no branch to offer, so the checkbox does not appear. Git can refuse a
  branch deletion (a branch checked out somewhere else, for instance), and when
  it does the worktree still goes and dux tells you the branch survived, quotes
  git's reason, and leaves the branch to you.
  A worktree that already has an agent names that agent and offers no delete,
  because removing it from under a live agent leaves a broken session. Delete
  the agent instead. This checkbox is also the manual override for a branch dux
  will not delete on its own: deleting an agent only ever removes branches dux
  created, so a branch that came from you outlives its agent and this dialog is
  where you can still remove it.

**Forking** is different: it starts from an *existing agent*, not the project.
Open that agent's `⋯` menu and pick **Fork agent…**, which opens the same New
agent dialog locked to fork mode. It copies the current files of that agent into
a new worktree and branch, so you can take a running piece of work in a second
direction without disturbing the first.

Whichever way you create an agent, it launches immediately, no separate start
step. The name input sanitizes itself to valid branch characters as you type.

## Managing an agent

An agent's `⋯` menu is where the rest lives: rename it (a display title, the
branch keeps its own name), change its provider, view its info, inspect its
project's environment and [startup command](/docs/startup-commands), and read
startup-command logs. **Change provider** and **Force recreate** are the two
knobs around resume. Change provider swaps which CLI the agent runs, but only
takes effect the next time that tab launches, never yanking a running session out
from under itself; that launch then follows the same per-provider resume rule as
any other launch, resuming the new provider's prior conversation on the worktree
only if it supports resume and no other live tab of the agent is already running
that same provider (Copilot never resumes, so a tab switched to Copilot always
starts fresh). Force recreate is the explicit "start clean, abandon the
conversation" button.

## Provider tabs

An agent can run more than one provider session in its single shared worktree, as
**tabs**. When an agent has two or more, a strip of Chrome-style pills appears
atop the terminal, each a live provider session against the same files. Add one
with the split **+** button (the main button uses the project's default provider,
the caret lets you pick), switch with a click, retarget or close from a pill's
`⋯` menu. Every tab is generic and uniform, there is no privileged "main" tab.

Resume is automatic and decided per provider, never a toggle you flip. A launching
tab resumes its provider's prior conversation when it is the only live tab of that
provider, and starts fresh otherwise, so a Claude tab and an OpenCode tab can both
resume side by side while two Claude tabs would not collide. Copilot is the one
exception: dux excludes it from resume entirely, so a Copilot tab always starts
fresh no matter what else is running. The full model, including how closing a tab
works and the per-agent tab cap, is in [Agent tabs](/docs/agent-tabs).

### Dormant tabs after a restart

When the server restarts, an agent's **extra** tabs come back **dormant**, not
running. dux does not restore a tab's conversation across a restart, so instead of
silently relaunching (and instead of quietly re-attaching, which would force a
launch), a dormant tab shows a card explaining it is not running with a **Start
session** button. Nothing launches on an extra tab until you ask it to. Opening the
agent's own tab is different: that view is the terminal, so opening it starts or
resumes the agent's provider. Your provider CLI likely still has
the conversation in its own history, so a started tab often picks up where it left
off, and every provider offers its own command to browse and choose a past
conversation if you want a specific one.

## Attention and notifications

You do not have to babysit every tab. When an agent needs you (a permission
prompt, a finished turn), its sidebar icon itself turns cyan and pulses, its
tab-strip pill gains a small cyan dot next to its icon, the browser tab title
gains a count like `(2) dux`, and the favicon grows a small cyan dot. The flag
clears the moment you actually look at that agent. The whole model, and how to
make sure your agents actually emit the signal, is covered in
[Attention indicators](/docs/attention-indicators).

Server mode can go one step further and raise a **real browser desktop
notification** when an agent asks for you, bridged from the agent's
own notification escape codes. The permission is strictly opt-in: dux never
auto-prompts for it. Open **Preferences…** from the cog menu and use **Enable
browser notifications** once, grant permission, and you are set. It fires only
while the dux browser window itself is hidden or unfocused, so dux never nags you
while you are looking at it. Only the agent whose view is open can raise one,
because that is the only PTY the browser is subscribed to. This is governed by the
`web_notifications` capability, detailed in
[Terminal capabilities](/docs/terminal-capabilities).

One caveat worth knowing: browsers only allow the notification-permission prompt
on secure origins, meaning HTTPS or `localhost`. If you reach dux over a plain-HTTP
Tailscale or LAN URL, **Enable browser notifications** can be silently blocked by
the browser before it ever shows a prompt. Loopback access or a TLS-terminating
proxy in front of dux gets you a working prompt.

## Kill versus delete

Two very different endings, and the difference matters:

- **Kill (detach)** stops the running session but keeps the agent and its
  worktree. It moves to a detached, reopenable state, and you can reconnect to it
  later. The **Task Manager**, in the app menu behind the cog button, is where you
  do this: it lists every running agent tab and companion terminal with its CPU,
  memory, and process count, expands any of them to show the child processes
  underneath, and stops them one by one or all at once. Every stop asks for
  confirmation first. Companion terminals, unlike agents, are **destroyed** when
  killed, not detached.
- **Delete** removes the agent from dux entirely. The confirmation includes an
  unchecked "also delete the git worktree" box, so by default your worktree and
  its work survive even a delete. What ticking it does to the branch depends on
  where that branch came from. For an agent whose branch dux created, both go:
  the branch the agent is on now, and the one it was created on if it has since
  moved. For an agent that attached to a branch that already existed, or that was
  adopted along with an existing worktree, only the worktree goes; the branch is
  yours, so dux keeps it and the confirmation says so before you tick anything.
  If git refuses to delete a branch dux did try to remove, dux says which branch
  is still there and why rather than reporting a deletion that did not happen.
  This is the one destructive per-agent action dux tints red, and it always
  confirms first.

Worktrees are your data, so dux never removes or mutates them casually. Deleting
an agent leaves its worktree on disk unless you explicitly opt in, which is
exactly why "adopt an existing worktree" above can bring one back.
