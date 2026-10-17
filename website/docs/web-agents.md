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

- **Not on its default branch.** dux offers to check the default branch out before adding.
  Leave the box ticked and new agents branch from the default branch; untick it and your
  folder stays where it is, and new agents branch from the branch it is on. The dialog names
  the branch either way.
- **No commits yet** (a fresh `git init`). dux offers to make the initial commit for you. If
  git on the server cannot work out an identity for that commit, dux fills in
  `dux@localhost` as the address so the add still succeeds; see
  [the workspace page](/docs/web-workspace) for what it keeps and what it fills in.
- **Not a repository at all.** dux offers to initialize one, as described next.
- **Inside another repository.** dux refuses and names the repository's root: add that
  repository instead.

The confirm button's label adapts to whichever applies.

A project's `⋯` menu carries everything you can do with it:

- **Start an agent**, plain or from a pull request.
- **Worktrees and terminals:** manage its worktrees, open a terminal at its root.
- **Branches:** pull it, check out its default branch, change its base branch.
- **Info and settings:** see its info, change its settings, read the startup-command logs of
  all its agents.
- **Leave:** delete or remove it.

Every project, including one with no agents yet, is in **Projects…** in the app menu; see
[the workspace page](/docs/web-workspace#managing-projects).

### Starting from an empty folder

**Initialize a repository…**, next to **Add project…** in the same menus, is for a folder
that is not a git repository yet. The dialog says what it will do: pick an existing folder
or create a new one with the new-folder control, then choose **Use this folder**.

dux then:

- runs `git init` in it;
- seeds a starter `.gitignore` covering the kinds of project it recognises in the folder (the
  dialog lists them, and leaves that out when none apply);
- makes an *empty* initial commit.

Your existing files are left alone and stay untracked, so nothing is committed on your
behalf. Picking a plain folder in **Add project…** offers the same step.

### Starting from a remote instead

**Clone a repository…**, next to **Add project…** in the same menus, takes a git address
rather than a folder. It asks for three things:

- the address;
- the destination folder (filled in from the address until you edit it);
- the first agent's name, with the same random-name box as **New agent**.

Press **Clone** and the dialog closes. Progress rides a toast, and the new agent is selected
when it shows up.

If dux can tell straight away that something is wrong, the dialog stays open with the reason
so you can fix it without retyping:

- no address;
- an agent name with characters a branch cannot hold;
- a destination that is not a full path, or whose parent folder does not exist;
- a folder that is already a project.

Everything else, including what each failure leaves on disk, is in
[Creating agents](/docs/creating-agents#cloning-a-repository).

## Creating an agent

Two doors, differing in what you start from.

**The launcher** sits at the bottom of the sidebar (and on the mobile hub): one filled
button and one `⋯`.

- **The button** is a single click to a plain new agent. It reads **Add project** instead
  while you have none.
- **The `⋯`** groups everything else you can create from nothing: **Agents** (from a pull
  request, from an existing worktree, or standalone in a folder you already have),
  **Terminals** (a standalone shell) and **Projects**.
- **The Agents header** above the list carries a **+** too.

> [!TIP]
> Use the launcher when you have not picked a project yet. It is the only place the
> reference-first flow lives.

A plain new agent starts by asking which project it belongs to. If the answer is "none",
the bottom of that project list has two ways out: **Add a new project…**, and **Add
standalone agent…**, which swaps the list for the folder picker a
[standalone agent](/docs/creating-agents#running-an-agent-in-a-folder-you-already-have)
starts from.

![The New agent project picker: a search field over the projects, each with its agent count, and Add a new project and Add standalone agent side by side at the bottom.](/screens/new-agent-picker.png)

**New agent from existing worktree…**, also in the launcher's `⋯`, picks a project and then
opens that project's worktree manager, described below, so you can bring an orphaned worktree
back as an agent.

### Starting from a pull request, with no project

Pick **New agent from PR…** from that `⋯` segment and the dialog opens with the reference
field first. **No project is chosen and none is asked for.** Paste a PR link (or
`owner/repo#123`, or a bare `owner/repo`) and dux compares the repository it names against
every project you have:

![The New agent from PR dialog with a pull request reference typed into its first field.](/screens/new-agent-from-pr-dialog.png)

- **One project is a checkout of that repository** and dux goes straight on to resolve the
  pull request.
- **Two or more are** and dux shows you just those and asks which.
- **None is**, and the reference names a repository and a pull request number on a host
  `gh` is signed in to. dux looks the pull request up and shows it, with its title and
  branch, and offers to clone the repository, add it as a project and start the agent on
  the pull request in one go.
  - If the folder it would clone into already holds a checkout of that repository, it offers
    to add that folder as a project instead, with no second download.
  - The folder and the agent name are editable, and **Choose an existing project** keeps the
    old path.
  - When the agent would need a review copy, the offer says so, with the same warning and
    unticked **Create a review copy** box as in a project, and asks before cloning or adding.
  - A reference without a number, or one `gh` cannot look up, gets the project picker with
    the repository named.

> [!IMPORTANT]
> **The clone is the same one as Clone a repository…** (see
> [Starting from a remote instead](#starting-from-a-remote-instead)). It uses the git
> protocol `gh` is set to for that host and never prompts for a password.
>
> Over SSH, connect to the host once in a terminal first (`ssh -T git@<host>`), or the clone
> cannot trust it.

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

Each row is in one of these states:

- **Unused.** It carries a `⋯` menu with **Delete worktree…**.
- **Has an agent.** The row names that agent and offers no delete. Delete the agent instead.
- **Being removed.** A worktree whose agent you just deleted is listed here until its removal
  finishes, with no delete of its own: it is already going.
- **In use.** Something is still using it but no agent is on it, and the row says what: an
  agent being created there, a terminal open in it, the CLI of an agent you just deleted with
  its worktree kept (still stopping), or a standalone agent whose folder is the worktree or
  anywhere inside it. It cannot be deleted from the manager until that has gone.

**Delete worktree…** removes the directory from disk after a confirmation naming the branch
and the full path. That confirmation carries an **"Also delete the branch"** checkbox, ticked
by default, because a branch left behind makes creating an agent under that name later fail
with "branch already exists".

- Untick it and dux removes the working directory only.
- A worktree that is not on a branch has no branch to offer, so the checkbox does not appear.
- If git refuses the branch deletion (it is checked out somewhere else, say), the worktree
  still goes and dux tells you the branch survived and quotes git's reason.

The manager checks again at the moment you confirm, and a few rules keep it safe:

- A worktree an agent took in the meantime, or one an agent is being created on, is refused
  rather than removed.
- A removal that finds work still running in the worktree waits for it the same way an agent
  delete does.
- A standalone agent inside a worktree keeps that worktree for as long as the agent exists,
  because dux never removes a standalone agent's folder. Delete that agent, or move its folder
  out, first. The same holds when you delete the worktree's own agent with the worktree box
  ticked: the worktree is kept and dux names the standalone agent.
- Once a removal of a folder has begun, nothing new starts in it or in any folder inside it:
  a new agent, a standalone agent, a relaunch, a terminal or a project added there is refused
  and says why.
- A worktree that holds a dux project's repository is never removed.

> [!TIP]
> This is where you remove the branch of a worktree that has no agent. For a worktree that
> does have one, use its agent's own delete dialog; see
> [Kill versus delete](#kill-versus-delete). Once a worktree is gone neither surface can reach
> its branch, and `git branch -D` is the way.

The terminal UI has the same manager, as the `manage-worktrees` palette command, and both
drive the same rules.

## Managing an agent

An agent's `⋯` menu is where the rest lives. It opens with the **Input** group (**Attach a
file…**, and the way back to the typing bar when you have turned it off), which is the same
group the pane's own `⋯` carries. The agent's actions sit below it:

- **Rename agent…** renames it, and its git branch too if you tick the box, exactly as in the
  terminal UI; see [renaming an agent](/docs/creating-agents#renaming-an-agent).
- **Change agent provider…** and **Force recreate agent…**, covered below.
- **New agent tab…**, **New terminal in the worktree** (or **in the folder**, for a
  standalone agent), and **Detach agent…** (only while something is running).
- **Agent info…**, **Open editor here** or **in new tab**, and **Copy local path**.
- **Recreate working copy…**, which only appears when the agent's folder has gone missing.
- The **auto-reopen** toggle, and **Delete agent…**.

These entries are about a branch dux manages, so a standalone agent does not get them at all:

- **Fork agent…**.
- The project's environment, [startup command](/docs/startup-commands), **Rerun startup
  command** and **Startup command logs…**.
- **Attach pull request…** (or **Change attached pull request…**), which needs the `gh` CLI.
- **Detach pull request**, when the agent has one, and **Resume PR autodetection**, which
  only appears after you have detached one.

![An agent's menu open beside its sidebar row: an Input group with Attach a file and Use virtual input at the top, then force recreate, detach and auto-reopen, then rename, fork, change provider, editor and delete actions.](/screens/agent-session-menu.png)

**Change provider** and **Force recreate** are the two knobs around resume:

- **Change agent provider…** swaps which CLI the agent runs, taking effect the next time that
  tab launches rather than yanking a running session. That launch follows the usual
  per-provider resume rule (see [how resume works](/docs/agent-tabs#how-resume-works)).
  Copilot never resumes, so a tab switched to Copilot always starts fresh.
- **Force recreate agent…** is the explicit "start clean, abandon the conversation" button.

## Provider tabs

An agent can run more than one provider session in its single shared worktree, as **tabs**.
With two or more, a strip of pills appears atop the terminal, each a live provider session
against the same files.

- **Add** one with the split **+** button: the main button uses the project's default
  provider, the caret lets you pick.
- **Switch** with a click.
- **Retarget or close** from a pill's `⋯` menu.

There is no privileged "main" tab: closing the agent's first tab hands its place to the next
tab in the strip, and only an agent's very last tab refuses to close (detach the agent
instead).

Resume is automatic and decided per provider, never a toggle: a Claude tab and an OpenCode
tab can both resume side by side, while two Claude tabs would not collide. Copilot is
excluded from resume entirely. The full model, including closing a tab and the per-agent tab
cap, is in [Agent tabs](/docs/agent-tabs).

### Dormant tabs after a restart

When the server restarts, every one of an agent's tabs comes back **dormant**, with no
process behind it.

- **An extra tab** shows a card saying it is not running, with a **Start session** button,
  and nothing launches until you ask.
- **The agent's first tab** is the exception: selecting the agent starts it, because asking
  for the agent is asking for that tab.
- **A tab whose last run ended badly** (it failed to launch, or the provider exited with an
  error) waits for you too, first tab included. It shows the same card, and only **Start
  session** retries it. That is deliberate, so a tab that cannot come up does not relaunch
  every time you open the agent.

Starting a dormant tab picks up that provider's conversation the same way any launch does;
see [how resume works](/docs/agent-tabs#how-resume-works). To reach an older conversation,
use the provider's own history command.

## Attention and notifications

When an agent needs you (a permission prompt, a finished turn), its sidebar icon turns cyan
and pulses, its tab-strip pill gains a small cyan dot, the browser tab title gains a count
like `(2) dux`, and the favicon grows a small blinking cyan dot. The flag clears the moment
you look at that agent. The whole model, and how to make sure your agents emit the signal, is
in [Attention indicators](/docs/attention-indicators).

Server mode can also raise a **real browser desktop notification** when an agent asks for
you:

- It is strictly opt-in: dux never auto-prompts. Open **Preferences…** from the cog menu, use
  **Enable browser notifications** once, and grant permission.
- It fires only while the dux browser window is hidden or unfocused, and only for the agent
  whose view you have open.
- It is governed by the `web_notifications` capability, detailed in
  [Terminal capabilities](/docs/terminal-capabilities#browser-notifications-web-ui).

> [!WARNING]
> Browsers only allow the notification-permission prompt on secure origins, meaning HTTPS or
> `localhost`. Over a plain-HTTP Tailscale or LAN URL, **Enable browser notifications** can
> be silently blocked before it ever shows a prompt. Loopback access or a TLS-terminating
> proxy gets you a working prompt.

## Kill versus delete

**Kill (detach)** stops the running session but keeps the agent and its worktree in a
detached, reopenable state. **Delete** removes the agent from dux entirely.

### The Task Manager

The **Task Manager**, in the app menu behind the cog button, lists every running agent tab
and every terminal (companion, project and standalone), with its CPU, memory, and process
count. Expand any row to see the child processes underneath. It also shows a row for dux
itself and a total.

- It stops rows one by one, or all at once. Every stop confirms first.
- Everything it stops, it stops **immediately**, with no shutdown wait, and each
  confirmation says so: it is the surface you reach for when the machine is already in
  trouble. **Detach agent…** is the polite alternative.
- **Force stop** on an agent's first row ends the agent and every tab it is running; on an
  extra tab it closes the tab; on a terminal it destroys the terminal.
- **Force stop everything…** does the same to every agent and terminal in one go. Both
  confirmations say so before you commit.

> [!WARNING]
> Terminals, unlike agents, are **destroyed** when killed, not detached.

### Detaching an agent

**Detach agent…** in the agent's `⋯` menu is the polite alternative to Force stop. You do not
have to be inside the agent's own CLI to use it. It asks every process the agent is running
to shut down, the same way closing dux does:

1. A polite signal first.
2. A wait.
3. A force-close for anything still there.

The agent stays in your list as **Detached** and you can resume it later. The terminal UI
reaches the same action with the `detach-agent` palette command on the selected agent.

- The entry only appears while the agent has something running, and it always confirms first.
- The confirmation names the wait, which is the top-level `shutdown_timeout_seconds` in your
  `config.toml` (30 seconds by default, and not the same key as the one under `[server]`).
- It also says how many of the agent's tabs stop together when it has more than one running.
- Afterwards dux says which way it went: that the agent shut down and is detached, or that it
  did not exit in time and was force-closed.

> [!CAUTION]
> Detaching interrupts work in progress. A CLI part-way through an edit or a command is
> given the shutdown grace to finish and then killed, so anything it had not written out is
> lost. There is no undo; you can start the agent again, but not the run you stopped.

### Deleting an agent

**Delete agent…** is the one destructive per-agent action dux tints red, and it always
confirms first. The confirmation includes an unchecked "also delete the git worktree" box, so
by default your worktree survives a delete.

Tick it and a second box appears, named after the branch. It only appears there because git
will not delete a branch that is still checked out in a worktree, so with the worktree
staying there is nothing to offer. Where the branch box starts depends on where the branch
came from:

- **Ticked**, for a branch dux created for the agent. Untick it to keep the branch.
- **Unticked**, for a branch that already existed, or that was adopted along with an existing
  worktree. Underneath it dux says which of the two it is, and adds "It has N commits not
  pushed anywhere" when there are any (in a repository with no remotes it says so
  differently, because there the count is the branch's whole history). Tick it and the branch
  goes anyway; this is the only way to remove such a branch from dux once its worktree is
  gone.

Edge cases:

- **The worktree moved onto another branch since the agent was created.** The tick removes
  **both**: the branch it is on now and the one it was born on, which is what keeps creating
  an agent under the old name from failing with "branch already exists". The box names both,
  dux says which one predates the agent, and the commit count covers the two together.
- **Git refuses to delete a branch dux tried to remove.** dux says which branch is still
  there and why, rather than reporting a deletion that did not happen.

### When somebody else is connected

Some changes end terminals, and somebody else may be looking at one: another browser tab, a
phone, or the terminal app. dux refuses these while anybody else is connected to what they
would end:

- deleting an agent, detaching it, force-stopping it, restarting it from scratch;
- stopping or closing a tab, closing a terminal;
- removing a project, recreating a working copy with its tabs restarted;
- starting an agent that would stop another agent already working in the same folder.

The refusal says who, and changes nothing. It gives:

- the device;
- the address it connects from (marked unverified when dux cannot vouch for it);
- whether they are typing in it or watching;
- which tab or terminal.

The delete, detach, force stop, force recreate, stop tab, close tab, close terminal, recreate
working copy and project dialogs say it in place, on both the browser and the terminal app,
and so do the browser's Force stop everything and the terminal app's Kill Running. In that
dialog:

1. The dialog stays open and lists everybody it would cut off.
2. Focus moves back to Cancel.
3. The button becomes **Delete anyway** (or **Detach anyway**, **Close tab anyway**, and so
   on). Pressing it goes ahead over exactly the people listed.
4. Cancel or Escape changes nothing.

Some details:

- If somebody new has connected by then, nothing happens and the dialog lists everybody
  again, so you never cut off someone you were not shown.
- A double click on the first button never presses the one that replaces it: the override
  takes its own click.
- In the terminal app the list also follows people connecting and leaving while the dialog
  is open.
- Quitting the terminal app while somebody else is connected to one of its terminals asks
  the same way, with **Quit anyway**.
- Only the connection asking is left out. The browser tab you are using is never in its own
  way, and neither is the terminal app deleting the agent it shows, but a second tab open on
  the same computer counts like any other device.
- The terminal app counts as connected to exactly the terminals it is drawing: an agent that
  is selected but not on screen, because a diff fills the center pane, protects nothing.

> [!IMPORTANT]
> **A phone in your pocket still counts.** When a browser tab was showing a terminal and that
> terminal's connection is lost rather than closed (the screen locked, the network dropped),
> the tab keeps counting as connected for `presence_grace_seconds` under `[server]` (300
> seconds by default), counted from the last moment it was on screen.
>
> It stops counting at once when the tab closes the terminal on purpose (you close the tab,
> navigate away, or open another agent or terminal in it), when it signs out, or when
> somebody goes ahead anyway. A device that was not showing the terminal stops counting once
> it has been silent for 75 seconds.

### What a delete with the worktree waits for

With the worktree box ticked, dux removes nothing until everything it started for that agent
has stopped:

- every tab;
- every terminal you opened on it;
- a startup command that is still running;
- anything any of them left running in the background, such as a dev server, a job started
  with `&`, or a `nohup`ed or disowned command.

Each is asked to exit, given the same shutdown grace as a quit (`shutdown_timeout_seconds`),
and killed if it is still there. The agent leaves the list at once; the removal finishes in
the background and reports when it is done.

It also waits for dux's own work in that worktree: a pull, a push, a commit, a branch rename,
a recreate of the working copy or a save from the editor still running there is allowed to
finish, and the status says what it is waiting for. A branch renamed while the delete waited
is removed under its new name.

From the moment you confirm, nothing new can start in that worktree: a save, a dropped file,
a pull or a new agent there is refused, and says why.

> [!WARNING]
> Ticking the box ends processes you may have started yourself in that agent's terminals,
> background jobs included, because the folder they work in is about to be deleted. Closing
> a terminal on its own still behaves like closing a terminal window: a disowned or
> `nohup`ed job keeps running.

When it cannot finish cleanly:

- **The work does not finish in time.** After `removal_wait_seconds` in your `config.toml`
  (120 seconds by default), dux keeps the worktree and tells you so. Remove it later from the
  worktree manager.
- **Something refuses to stop even when killed.** dux keeps the worktree untouched and names
  the processes, so you can stop them and remove the worktree from the worktree manager.
- **A terminal is standing in it.** A project or standalone terminal whose shell you `cd`'d
  into the worktree, or anything else dux started elsewhere that is standing in it, is never
  ended to make way. The worktree is kept and dux names the terminal: `cd` out of the folder
  or close the terminal, then remove the worktree from the worktree manager.
- **Something dux cannot see writes into it.** A server another program launched in that
  folder can still write while git deletes it. git then removes the worktree from the
  repository but cannot delete the folder. dux does not finish the job by deleting the folder
  itself; it tells you what is left in it and that you can delete the folder once whatever is
  writing there has stopped.

> [!IMPORTANT]
> dux only sees what it started. A program that fully detaches itself from the terminal
> session and the process tree, such as a `tmux new -d` session or a daemon that double-forks,
> is not seen at all: dux neither stops it nor counts it as using the worktree.
>
> Stop such a program yourself before you delete the worktree.

If you quit dux while a removal is still waiting, the quit finishes it before exiting. If dux
is killed or crashes first, the next start finishes it in the background and says so. It
never removes a folder that another agent has started using in the meantime.

> [!IMPORTANT]
> Worktrees are your data. Deleting an agent leaves its worktree on disk unless you explicitly
> opt in, which is why starting an agent from an existing worktree (see
> [the worktree manager](#the-worktree-manager)) can bring one back.
