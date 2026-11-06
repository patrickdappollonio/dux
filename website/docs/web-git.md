---
title: Git without leaving the browser
description: The Changes pane in server mode, with staged and unstaged groups, stage, unstage, discard, commit, push, pull, the PR banner and an agent's pull requests, and how the pane stays in sync.
group: Web UI
order: 63
---

The Changes pane is where you review what an agent did, and it does the everyday git
chores too, so you rarely need to drop to a shell.

## The Changes pane

The right-hand Changes pane tracks the focused agent's working tree. It is the place to review what an agent did.

- **Two groups.** Files split into collapsible **Staged** and **Unstaged** groups, each with a count badge.
- **A filter box** narrows a long list by path.
- **Every row** leads with its status icon, then the file path and the green and red line counts.
- **A clean worktree** says so plainly.

### Counts and markers

Each group heading, and the pane's own header, adds up the line counts of the rows below it. The figures follow the filter, describing exactly the rows you can see.

- **Big sums shorten.** A sum of a thousand lines or more reads in thousands, rounded down so it never overstates: 1,300 lines is "1.3k" and 12,345 is "12.3k". Only sums shorten; the counts on the rows always show every digit.
- **bin** marks a binary file, which has no lines to count. Binary files are tallied separately.
- **Excl** marks a tracked file your repository flags `-diff` in a `.gitattributes`. Git will not diff it, so it has no counts and gets its own "excl" tally, so it is never mistaken for a binary. It is ordinary text, and clicking the row still opens the diff.
- **New, untracked files** are counted line by line whatever `.gitattributes` says.

> [!NOTE]
> Git has never seen untracked files, so dux counts their lines by reading each file, and it does that for the first **two thousand** untracked files in a worktree. Any beyond that are still listed, in full, with their status. They just carry no line counts and are left out of the sums, the same way an empty file already looks.

Status icons are the same ones the editor's file tree uses, each with a tooltip spelling it out:

- **M** modified
- **A** added
- **D** deleted
- **R** renamed
- **C** copied
- **U** conflict
- **T** type changed
- **?** untracked

### Folders

A folder with nothing tracked inside it, like a freshly installed `node_modules`, is **one row** rather than one row per file, the way `git status` shows it: its path with a trailing slash and how many files it holds, for example `node_modules/  28,747 files`.

- Files your `.gitignore` hides are not counted.
- Stage it whole and it stays one row in **Staged**, for as long as everything in it is newly added and no file inside it is still untracked. A repository left out of the stage does not count against this. Stage only part of it and the staged files are listed one by one.
- Every count of changed files, the group badges included, counts the files *inside* a folder, not one for the folder.
- A git repository of its own inside a folder is counted apart, as in `3 files and 1 nested repository`. A folder that is itself a repository says "nested repository" instead of a count, because dux does not look inside it.
- A bare repository is different: to git it is a folder of ordinary files, so dux counts and stages it as files, the way `git add` would. Discarding a folder keeps any bare repository inside it, and the dialog names it.

**Opening a folder.** A folder row has no diff to open. Click its name to expand it and list what is inside, one level at a time; click again to fold it back up.

- Folders inside are folded and expand the same way.
- Every file inside is a full row: click it for its diff, check it, or stage, unstage or discard it from its own `⋯`.
- What you have expanded is remembered for each agent until you reload the page.
- It follows the files as they change: an expanded folder that gains or loses files lists itself again on its own, at any depth, and one that is gone is folded away.
- Collapsing a folder unchecks the rows inside it.
- A folder whose contents could not be listed says why and offers **Retry**. If a refresh fails, the rows from before stay, and the folder says it couldn't refresh and offers **Retry**.

**From the keyboard**, the list is one stop in the Tab order and a row holds the focus:

- The **arrow keys** move between rows. Right opens a folder (or steps into one already open); Left folds it (or steps back to the folder above).
- **Home** and **End** jump to the first and last row.
- **Enter** does what a click on the row does: a file's diff, or a folder opening or folding.
- **Space** checks the row.
- **Tab** from a row goes through that row's own controls (the checkbox, the folder's name and its menu) and then out of the list.

> [!NOTE]
> In the terminal UI, the diff key expands a folder row a level at a time and collapses it again, where the browser uses a click on the folder's name.

**Staging and unstaging a folder.** A folder row's own `⋯` menu stages or unstages the whole folder at once.

- Staging stages its files and leaves out any repository of its own, or worktree of this repository, inside it, so none of them is recorded as a link.
- dux tells you what it left out, and the folder's staged row says so too, as in `5 files · 1 nested repository not staged`.
- A folder that holds nothing but repositories has nothing to stage, so it offers no stage.

**Discarding a folder** asks first, the same confirmation a file gets.

- Discarding an untracked folder deletes the untracked files it counted. Files your `.gitignore` hides stay, and so does any repository of its own inside it; the dialog says so.
- A folder that holds nothing but repositories of their own has nothing to delete, so it offers no discard.
- A folder that is a worktree of this same repository is left to the worktree manager and offers no discard either.
- A folder or repository is **never deleted while something lives in it**: another agent's worktree or a standalone agent's folder, a dux project's repository, an operation dux is running there, something dux started that is still running there, or a terminal whose shell is standing in it. The discard is refused and says which, and nothing is deleted.

Discarding a deleted tracked file restores it from the last commit, unless a folder now stands where the file was. Then the discard is refused, because restoring the file would delete that folder and everything in it. Move or delete the folder yourself first if that is what you mean.

> [!CAUTION]
> Discarding a row that is a repository of its own deletes that whole repository, including its history and any commits you have not pushed anywhere else. The dialog warns you first.

### Opening a diff

Click any row to open its diff in the [code editor](/docs/web-editor), read-only, HEAD against the working copy, and syntax-highlighted below the ceiling described next.

The browser has a ceiling of its own, rather than the terminal UI's. When a version is over **5 MiB** on either side, the editor shows the start of git's own diff, coloured by added and removed lines, up to **4,000 lines** or about **2 MiB** of patch text, whichever comes first.

- When there is more than it shows, a line above it says how many lines the whole diff has.
- A binary file keeps the same one-sentence summary it always had, since there is no text diff to cut.

> [!IMPORTANT]
> Past that ceiling you are reading the start of the change, not all of it. Open the file in your editor, or run git diff yourself, to see the rest.

## Several files at once

Hover a row and its status icon turns into a checkbox. Tick it and a bar appears above the list with the verbs for what you picked: **Stage 3**, **Unstage 2**, **Discard 3…**, **Select all**, and **Clear**. On a touch screen there is no hover, so tapping the status icon is what ticks the row.

- **Each section keeps its own selection**, because the two carry opposite verbs.
- **Select all** picks every row currently on screen, in both sections, and flips to **Select none** once they are all ticked.
- **Clear** is the wider one: it empties the whole selection, including rows a filter is hiding.

![Two changed files ticked, with a bar above the list offering Stage 2, Discard 2, Select all and Clear.](/screens/changes-bulk-bar.png)

Staging or unstaging a selection is one operation with one message telling you what happened. If a file moved out of its group between the tick and the click, the rest still go through and the message names what was skipped.

Discarding a selection confirms first, and the dialog counts the two outcomes separately: how many untracked files it will delete, and how many tracked files it will restore.

> [!TIP]
> The filter and the selection are independent. Filter the list, tick what you want, clear the filter and tick some more: the bar still counts and acts on everything you picked, not just the rows on screen.

## Stage, unstage, discard

Clicking a row still opens its diff; only the checkbox selects. On a phone the diff, and **Edit** below, open in the standalone editor in the same browser tab, and Back returns to the Changes screen. For a single file, hover a row for its `⋯` menu:

- **Stage** and **Unstage** move a file between the two groups. The row jumps to its new group as soon as dux confirms.
- **Edit** opens the file in the editor (hidden for deleted files).
- **Discard…** throws away a file's uncommitted changes. It only shows up on **unstaged** rows: unstage a file first if you want to discard it.

> [!CAUTION]
> **Discard is destructive**, so it always confirms first, and the dialog says which case applies. An untracked file is **permanently deleted from disk**. A tracked file is **restored to its last committed state**. dux checks the file's live state at the moment you confirm.

## Commit, push, pull

The changes pane's header carries two controls: an **Open editor** button that takes you straight to the editor for the agent whose changes you are looking at (in the page on a computer, in the editor's own tab on a phone), and the `⋯` menu with the rest:

- **Commit…** opens a dialog for a multi-line message and commits **only the staged files**. It is disabled until something is staged, and `Cmd/Ctrl+Enter` submits.
- **Push** and **Pull** are one click each, with a progress toast that reports back to the browser tab you triggered them from.
- **Refresh changes** asks git again on the spot and reports what it found.
- **Hide Changes pane** tucks the whole pane away.

**Hiding and bringing the pane back.** Dragging the split all the way closed does the same as **Hide Changes pane**, though only when you let go: drag past the snap and back out before releasing and the pane stays. Either way:

- A button appears on the right of the header to bring it back at a sensible width, carrying the agent's changed-file count so you can still see at a glance how much has moved.
- The **Show the Changes pane** row in **Preferences** is the other way in, since hiding the pane takes this menu with it.

**The header stays in every state the pane can be in:** while the changes load, when they fail to, and when the pane has nothing to show because a standalone agent's folder has no repository or an agent's directory is gone.

- **Commit…** and **Refresh changes** stay in the menu greyed out, with a line at the top saying why. They come back on their own once the folder holds a repository again; there is nothing to press, because dux looks at the folder again within about ten seconds while the agent is selected.
- **Push** and **Pull** are greyed out the same way when a managed agent's working copy is gone. A [standalone agent](/docs/creating-agents) never has them, so they are absent from its menu rather than greyed out.
- **Hide Changes pane** and the **Open editor** button keep working, except that the editor button is greyed out too when the directory itself is gone.

## The PR banner

> [!NOTE]
> The banner, and attaching a pull request by hand, go through the `gh` CLI, so they cover
> GitHub and GitHub Enterprise hosts `gh` is signed in to. Commit, push and pull work with any
> host git can reach.

An agent can have several pull requests: the ones dux finds on every branch its worktree
has been on, plus any you attach yourself. The banner is a one-line strip that shows one of them at a
time: its number, its state and its title, color-coded (open is green, draft is gray,
merged is purple, closed is red). Clicking the strip opens that pull request in a new tab.
You can move the banner above or below the terminal in **Preferences** (the cog menu).

![A green strip under the terminal showing pull request 123, its open state and its title, with "1 of 3", two arrows and a ⋯ button at its right end.](/screens/pr-banner.png)

The right end of the strip holds its controls, which are not part of the link:

- **"1 of 3" and the ‹ › arrows**, once the agent has two or more. They step through the
  agent's pull requests in its [order](#ordering) and wrap around at either end. Stepping
  only changes what this banner shows; it changes nothing on the agent.
- **The `⋯` button**, always there. It lists every pull request the agent shows, each with
  its state and the branch it was found on (or "attached by you"), and a click on one opens
  it. **Pull requests…** at the bottom opens the dialog below.

![The banner's ⋯ list open over the terminal: three pull requests, the open one on fix-login-redirect, a draft attached from acme/infra and a merged one on an earlier branch, with Pull requests… underneath.](/screens/pr-banner-list.png)

On a phone the terminal screen's top bar carries a chip, such as **#123 +2**: the pull
request the banner shows, and how many more the agent has. With more than one, a tap on the
chip opens the same list as a sheet; with one, it opens that pull request.

In the sidebar, an agent with more than one pull request wears a count beside the
pull-request icon, colored by the one its banner shows first. Clicking the badge opens that
one, and its tooltip says how many more there are.

> [!NOTE]
> There is no "create a PR" button in the web UI. The banner surfaces **existing** PRs;
> agents open PRs themselves as part of their work. Pulling a PR's branch into a fresh
> agent is covered in [Agents from the browser](/docs/web-agents).

### The Pull requests dialog

**Pull requests…** in the agent's ⋯ menu (or at the bottom of the banner's list) opens one
dialog for everything about an agent's pull requests. Its **Pull requests** view lists the
ones the agent shows, in its order, the first being the one the banner shows. Each row's ⋯
has:

- **Open on GitHub**.
- **Move to top**, **Move up** and **Move down**.
- **Hide**, for a pull request dux found, or **Detach**, for one you attached. Neither asks
  for confirmation, because neither loses anything: see [Hiding and
  detaching](#hiding-and-detaching).

![The Pull requests dialog: three pull requests with their states and branches, a grip on each row for dragging, the Sort control beside the Pull requests and Branches tabs, and Attach pull request… at the bottom.](/screens/pull-requests-dialog.png)

On a phone the dialog is a sheet, and the second line of each row keeps the state and the
branch so the title has the room.

![The Pull requests sheet on a phone, listing the same three pull requests with a grip and a ⋯ on each row.](/screens/phone-pull-requests-sheet.png)

In the terminal UI the same list is the `pull-requests` palette command. It opens a picker
where Enter opens the selected pull request, and the picker names the keys for everything
else (attach, hide or detach, show the hidden ones, the branches, the order, the moves),
each of which can be rebound like any other key. The terminal UI's banner is a band above or
below the agent's terminal that shows one pull request with a count such as "1/3", and Ctrl-n
steps it to the next one while the agent pane is not taking your typing; while it is,
Ctrl-n goes to the agent, where shells and most CLIs use it for their history.

![The terminal UI with its pull-request band under the agent's terminal, showing pull request 123, its title, 1/3 and Ctrl-n next.](/screens/tui-pr-band.png)

![The terminal UI's Pull Requests picker: the open pull request and the attached draft under Open, newest first, the merged one under Merged and closed, and the keys for each action along its top.](/screens/tui-pull-requests-modal.png)

### Attaching a PR by hand

dux finds the pull requests on the agent's branches itself. When one lives somewhere it
does not look (a fork, another repository entirely, or a head branch that no longer
matches), attach it: **Attach pull request…** at the bottom of the Pull requests dialog, or
the attach key in the terminal UI's picker. The field takes a full PR URL, `owner/repo#123`,
or a bare number resolved against the project's remote.

Attaching **adds** to the agent's list; it never replaces a pull request it already has,
and autodetection keeps running beside it. A pull request from another repository works
too, and the list names it with its repository (`acme/infra#77`) so it is never confused
with one of the agent's own. Agent Info lists the pull requests the agent shows, not the ones you hid, in the browser
and the terminal UI alike, and marks the ones you attached as manually attached.

### Hiding and detaching

Every pull request in the list can be taken out of it, one at a time:

- **Hide** is for a pull request dux found. It leaves the banner and the list for a
  **Hidden** section at the bottom of the dialog, and dux never brings it back by itself.
  **Show again** there puts it back in the place it had.
- **Detach** is for one you attached. It goes from the agent; attach it again whenever you
  like. One dux had also found stays, as found, and can then be hidden.

A pull request dux has found is never dropped by dux itself: a branch nobody pushes to any
more keeps its pull request until you hide it.

Attaching needs GitHub integration and a signed-in `gh`; hiding, showing again and
detaching do not, so an association never outlives your ability to remove it. With GitHub
integration off entirely, the banner and the badge are hidden until you switch it back on.

### Ordering

The **Sort** control beside the dialog's two views has two settings:

- **Automatic**, the default: open first, then drafts, then merged, then closed, the
  newest first within each.
- **Manual**: your own order, with the pull requests dux finds later going after the ones
  you placed, in automatic order among themselves.

Dragging a row by its grip, or **Move to top**, **Move up** or **Move down** in its ⋯,
switches to Manual. A move made while the order is Automatic starts from the order on
screen and replaces any manual order you had saved before; the toast says so. Choosing
Automatic goes back without forgetting your order, and choosing Manual again brings it back
as you left it. A hidden pull request keeps its slot and returns to it when shown again.

### Branches

The dialog's **Branches** view lists every branch the agent's worktree has had checked
out, the current one first and the rest newest first, each with when dux first saw it, the
names it had before a rename, and the pull requests found on it. Every other pull request
the agent has is listed below the branches, so none is missing from the view: one you
attached, one dux found on a branch the list leaves out, such as the default branch
after the worktree moved off it, and one found in another repository before the worktree's
remote changed, even on a branch of the same name. The terminal UI puts them under **On no branch listed
here**.

![The Branches view: fix-login-redirect marked Current with pull request 123, an earlier branch with its merged pull request, and the attached acme/infra#77 under its own heading.](/screens/pull-requests-branches.png)

- **What is tracked:** every branch in that list. The branch checked out now is asked
  about on every poll and the earlier ones take turns; see
  [How PR status stays fresh](/docs/creating-agents#how-pr-status-stays-fresh).
- **When history starts:** when dux first saw the worktree on a branch. A branch it was on
  before dux was watching is not in the list.
- **The project's default branch** (usually `main`) is listed and tracked only while the
  worktree is on it. Once the worktree moves off it, it leaves the list and dux stops
  looking for pull requests on it, because its pull requests are everybody's, not the
  agent's; one dux already found there stays in the agent's list.

> [!NOTE]
> The first start of this version moves every agent's pull request into the new list. If
> you go back to an older release afterwards, it shows the pull requests as they were at
> that moment, and changes you make while running the older release are not carried
> forward when you upgrade again.

## Staying in sync

There is no file watcher behind the Changes pane.

The pane updates the moment dux itself changes a file: a stage, an unstage, a discard, a commit, a file saved in the editor, and a file you drop onto a pane once it lands somewhere git is watching.

Everything else is noticed within a couple of seconds while any agent or terminal is running, and within ten seconds when none is: an agent writing files in its worktree just as much as a file you delete from a companion terminal.

> [!IMPORTANT]
> A change dux did not make is never invisible, it is just up to ten seconds late. **Refresh changes** in the header menu skips that wait and says what it found. The terminal UI has the same action as its `refresh-changes` command.

If a git operation collides with a lock, dux keeps retrying, so a single blip usually clears itself. If it does not, you get a "Couldn't load changes" card with a Refresh button, and a warning toast once the failures persist. A commit you make in the browser is an ordinary commit, visible everywhere.

## When an agent deletes its own working copy

An agent works inside a git worktree, and it can remove that worktree from inside it: merging its branch and then deleting the branch takes the directory with it. dux cannot stop that, so it says so instead. The Changes pane goes quiet with a sentence naming the directory that is gone, and the agent's row in the sidebar says **working copy missing** where it would normally name the project.

> [!IMPORTANT]
> The agent's process, if it was still running, keeps running in a directory that no longer exists. Nothing it does from then on reaches your files.
>
> Recreating puts the directory back, **not its contents**: the code changes that were in the old directory are gone.

### Recreating the working copy

**Recreate working copy…** in the agent's `⋯` menu checks the agent's branch out again at the *same path*. That is what lets an agent whose CLI keeps its history per directory resume the same conversation. The terminal UI has the same action as its `recreate-working-copy` command.

Where the branch comes from:

- If the branch is still in the repository, dux checks it out again and the agent's commits are exactly where it left them.
- If the branch is gone locally but still on the remote, dux creates it again from `origin/<branch>`, holding everything that had been pushed.
- If it is gone everywhere, dux creates it again from the project's source branch, and the commits that branch held are not coming back.

The confirmation spells out all three cases, and the message afterwards names the one that actually happened.

**If the agent is still running**, you can recreate anyway; what happens next is the CLI's own answer. dux tells the CLIs apart by the command a provider runs, not by the provider's name, so a provider of your own whose command is `claude` counts as Claude.

- **Claude** never notices. A running Claude tab carries on in the recreated folder with its conversation intact. dux treats **Grok** the same way, which it has not measured separately.
- **Codex** cannot follow the folder. It answers every turn with an invalid-directory error from the moment the folder goes, and stays stuck after the recreate until it is restarted.
- **OpenCode**, and any other CLI that can pick its conversation back up, gets the same cautious treatment as Codex.
- **Copilot**, and any CLI that cannot pick a conversation back up, is never restarted, because that would lose its conversation. The confirmation says it is left running. If it cannot follow the folder, stop and start that tab yourself.

**The restart box.** For the cautious CLIs, the confirmation has a **Restart the Codex tab so it continues in the new copy** box, ticked by default and naming the CLIs it would restart. Leave it ticked and, once the working copy is back:

1. dux asks those tabs to shut down.
2. It gives them the configured shutdown wait, and closes any that are still there.
3. It starts each one again so it picks its conversation back up in the recreated folder.

A few rules apply:

- Only one tab per CLI can get its conversation back, so with two Codex tabs the second starts a fresh one.
- Each tab comes back with the CLI it was running, even if you picked a different one for its next start, because the conversation belongs to that CLI.
- Restarting stops tabs, so if someone else is connected to one of them, the confirmation names them first, the same way stopping a tab does.
- Nothing is restarted if the recreate fails, and nothing at all when the box is unticked.

**While a tab waits to be restarted**, starting it yourself is refused with a message saying dux is restarting it. Stopping or closing that tab, stopping the agent, or deleting it means it is not started again. A tab that only started after you confirmed is left running, and the message says so.

**The message afterwards** waits until every restarted tab has actually started and, for one picking its conversation back up, until it has shown it did. Then it says which continued, which started fresh (including one whose conversation could not be picked back up), which were stopped, and which could not be started and why.

While the working copy is gone, the editor and a new terminal for that agent are refused with the same sentence: there is no directory for either to open.
