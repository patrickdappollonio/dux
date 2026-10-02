---
title: Git without leaving the browser
description: The Changes pane in server mode, with staged and unstaged groups, stage, unstage, discard, commit, push, pull, the PR banner, and how the pane stays in sync.
group: Web UI
order: 63
---

The Changes pane is where you review what an agent did, and it does the everyday git
chores too, so you rarely need to drop to a shell.

## The Changes pane

The right-hand Changes pane tracks the focused agent's working tree. Files split into two
collapsible groups, **Staged** and **Unstaged**, each with a count badge, and a filter box
narrows a long list by path. Every row leads with its status icon, then the file path and
the green and red line counts. Each group heading, and the pane's own header, adds those
counts up for the rows below it, so you can see at a glance how much has moved; binary
files have no lines to count, so they are tallied separately as a quiet "bin" marker. A
file your repository marks `-diff` in a `.gitattributes` has no counts either, because git
will not diff it, and it gets its own quiet "Excl" marker and its own "excl" tally so it is
never mistaken for a binary: the file is ordinary text and clicking the row still opens the
diff. A
summed-up figure of a thousand lines or more is shortened to read in thousands, rounded
down so it never overstates: 1,300 lines reads "1.3k" and 12,345 reads "12.3k". Only these
sums shorten; the counts on the rows themselves always show every digit. The figures follow
the filter, describing exactly the rows you can see. When the worktree is
clean it says so plainly.

Untracked files are the one place with a ceiling on the counting. git has never seen them,
so their lines have to be counted by reading each file, and dux does that for the first two
thousand untracked files in a worktree. Any beyond that are still listed, in full, with
their status; they simply carry no line counts and are left out of the sums, the same way
an empty file already looks.

A folder with nothing tracked inside it, like a freshly installed `node_modules`, is one
row rather than one row per file, the way `git status` shows it: its path with a trailing
slash and how many files it holds, for example `node_modules/  28,747 files`. Files your
`.gitignore` hides are not counted. Stage it whole and it stays one row in **Staged**, for as
long as everything in it is newly added and no file inside it is still untracked (a
repository left out of the stage does not count); stage only part of it and the staged files
are listed one by one. Every count of changed files, the
group badges included, counts the files inside a folder rather than one for the folder. A
git repository of its own inside a folder is counted apart, as in `3 files and 1 nested
repository`, and a folder that is itself a repository says "nested repository" instead of a
count, because dux does not look inside it. A bare repository is different: to git it is a
folder of ordinary files, so dux counts and stages it as files, the way `git add` would.
Discarding a folder keeps any bare repository inside it, and the dialog names it.

A folder row has no diff to open. Click its name to expand it and list what is inside, one
level at a time; click it again to fold it back up. The folders inside are folded the same
way and expand the same way, and every file inside is a full row: click it for its diff,
check it, or stage, unstage or discard it from its own `⋯`. A folder whose contents could
not be listed says why and offers Retry. What you have expanded is remembered for each agent
until you reload the page, and it follows the files as they change: an expanded folder that
gains or loses files lists itself again on its own, at any depth, one that is gone is folded
away, and collapsing a folder unchecks the rows inside it. If a refresh cannot be listed,
the rows from before stay, and the folder says it couldn't refresh and offers Retry. From
the keyboard, the list is one stop in the Tab order and a row holds the focus: the arrow
keys move between rows, Right opens a folder (or steps into one already open) and Left folds
it (or steps back to the folder above), Home and End jump to the first and last row, Enter
does what a click on the row does (a file's diff, a folder opening or folding), and Space
checks the row. Tab from a row goes through that row's own controls, the checkbox, the
folder's name and its menu, and then out of the list.

A folder row's own `⋯` menu stages or unstages the whole folder at once, and discards it
after a confirmation, the same one a file gets. Staging a folder stages its files and leaves
out any repository of its own, or worktree of this repository, inside it, so none of them is
recorded as a link; dux tells you what it left out, and the folder's staged row says so too,
as in `5 files · 1 nested repository not staged`. A folder that holds nothing but
repositories has nothing to stage, so it offers no stage. Discarding an untracked folder
deletes the untracked files it counted: files your `.gitignore` hides stay, and so does any
repository of its own inside it, and the dialog says so. A folder that holds nothing but
repositories of their own has nothing to delete, so it offers no discard. A folder that is a
worktree of this same repository is left to the worktree manager and offers no discard
either.

> [!CAUTION]
> Discarding a row that is a repository of its own deletes that whole repository, including
> its history and any commits you have not pushed anywhere else. The dialog warns you first.

> [!NOTE]
> In the terminal UI, the diff key expands a folder row a level at a time and collapses it
> again, where the browser uses a click on the folder's name.

Click any row to open its diff in the [code editor](/docs/web-editor), read-only, HEAD
against the working copy, and syntax-highlighted below the ceiling described next. The
browser has a ceiling of its own rather than the terminal UI's: when a version is over
**5 MiB** on either side, the editor shows the first **4,000 lines** of git's own diff, coloured
by added and removed lines. When there is more than it shows, a line above it says how many
lines the whole diff has. A binary file keeps the same one-sentence summary it always had,
since there is no text diff to cut.

> [!IMPORTANT]
> Past that ceiling you are reading the start of the change, not all of it. Open the file in
> your editor, or run git diff yourself, to see the rest.

Status icons are the same ones the editor's file tree uses, each with a tooltip spelling it
out: **M** modified, **A** added, **D** deleted, **R** renamed, **C** copied, **U**
conflict, **T** type changed, and **?** untracked.

## Several files at once

Hover a row and its status icon turns into a checkbox; tick it and a bar appears
above the list with the verbs for what you picked: **Stage 3**, **Unstage 2**,
**Discard 3…**, **Select all**, and **Clear**. On a touch screen there is no
hover, so tapping the status icon is what ticks the row. Each section keeps its
own selection, because the two carry opposite verbs.

**Select all** picks up every row currently on screen, in both sections, and
flips to **Select none** once they are all ticked. **Clear** is the wider one:
it empties the whole selection, including rows a filter is hiding.

![Two changed files ticked, with a bar above the list offering Stage 2, Discard 2, Select all and Clear.](/screens/changes-bulk-bar.png)

Staging and unstaging a selection is a single request: one git call, one refresh,
one message telling you what happened. If a file moved out of its group between
the tick and the click, the rest still go through and the message names what was
skipped. Discarding a selection confirms first, and the dialog counts the two
outcomes separately: how many untracked files it will delete, and how many
tracked files it will restore.

> [!TIP]
> The filter and the selection are independent. Filter the list, tick what you
> want, clear the filter and tick some more: the bar still counts and acts on
> everything you picked, not just the rows on screen.

## Stage, unstage, discard

Clicking a row still opens its diff; only the checkbox selects. For a single
file, hover a row for its `⋯` menu:

- **Stage** and **Unstage** move a file between the two groups. The row jumps to its new
  group as soon as dux confirms.
- **Edit** opens the file in the editor (desktop only, and hidden for deleted files).
- **Discard…** throws away a file's uncommitted changes. It only shows up on **unstaged**
  rows, both in the menu and on the server: unstage a file first if you want to discard it.

> [!CAUTION]
> **Discard is destructive**, so it always confirms first, and the dialog says which case
> applies. An untracked file is **permanently deleted from disk**. A tracked file is
> **restored to its last committed state**. The server re-derives which from live git
> status at the moment you confirm.

## Commit, push, pull

The changes pane's header carries two controls: an **Open editor** button that takes you
straight to the editor for the agent whose changes you are looking at (in the page on a
computer, in the editor's own tab on a phone), and the `⋯` menu with the rest:

- **Commit…** opens a dialog for a multi-line message and commits **only the staged
  files**. It is disabled until something is staged, and `Cmd/Ctrl+Enter` submits.
- **Push** and **Pull** are one click each, with a progress toast that reports back to the
  browser tab you triggered them from.
- **Refresh changes** asks git again on the spot and reports what it found.
- **Hide Changes pane** tucks the whole pane away. Dragging the split all the way closed
  does the same, though only when you let go: drag past the snap and back out before
  releasing and the pane stays. Either way a button appears on the right of the header to
  bring it back at a sensible width, carrying the agent's changed-file count so you can
  still see at a glance how much has moved, and the **Show the Changes pane** row in
  **Preferences** is the other way in, since hiding the pane takes this menu with it.

The header stays in every state the pane can be in: while the changes load, when they
fail to, and when the pane has nothing to show because the agent's folder has no
repository or its directory is gone. In those quiet states **Commit…**, **Push**,
**Pull** and **Refresh changes** stay in the menu greyed out, with a line at the top
saying why, and come back on their own once the folder holds a repository again.
**Hide Changes pane** and the **Open editor** button keep working, except that the
editor button is greyed out too when the directory itself is gone.

## The PR banner

When a session is tied to a GitHub pull request, a one-line strip shows the PR number, its
state, and its title, color-coded (open is green, merged is purple, closed is red).
Clicking it opens the PR in a new tab. You can move the banner above or below the terminal
in **Preferences** (the cog menu).

![A green strip under the terminal showing pull request 123, its open state and its title.](/screens/pr-banner.png)

> [!NOTE]
> There is no "create a PR" button in the web UI. The banner surfaces an **existing** PR;
> agents open PRs themselves as part of their work. Pulling a PR's branch into a fresh
> agent is covered in [Agents from the browser](/docs/web-agents).

### Attaching a PR by hand

dux normally finds the PR itself by matching the agent's branch name against the repository
on GitHub. When that misses (the PR lives on a fork, or under a head branch that no longer
matches), attach one by hand: **Attach pull request…** in the agent's ⋯ menu, or the
`attach-pull-request` command in the terminal UI's command palette. The field takes a full
PR URL, `owner/repo#123`, or a bare number resolved against the project's remote.

A manually attached PR is pinned: its state still refreshes, autodetection stops
second-guessing it, and the Agent Info dialog says "manually attached".

### Detaching, and getting detection back

**Detach pull request** (same menu, or `detach-pull-request` in the palette) means this
agent has no pull request. The badge goes immediately, a pin is dropped if there was one,
and dux stops looking for a PR on the agent's branch. It works on an autodetected PR too,
not only on a pin, and the detach is remembered across restarts.

Two things bring detection back: attaching a PR by hand, or **Resume PR autodetection**,
which appears in the agent's ⋯ menu (and as `resume-pull-request-autodetection` in the
palette) only while the agent is detached and checks GitHub straight away. Neither
detaching nor resuming is destructive, so neither asks for confirmation.

Attaching needs GitHub integration and a signed-in `gh`; detaching and resuming do not, so
an association never outlives your ability to remove it. With GitHub integration off
entirely, the pin is hidden and not removable until you switch it back on, at which point
the badge comes back.

## Staying in sync

There is no file watcher behind the Changes pane.

The pane updates the moment dux itself changes a file: a stage, an unstage, a discard, a
commit, a file saved in the editor, and a file you drop onto a pane once it lands somewhere
git is watching.

Everything else is noticed within a couple of seconds while any agent or terminal is
running, and within ten seconds when none is: an agent writing files in its worktree just
as much as a file you delete from a companion terminal.

> [!IMPORTANT]
> A change dux did not make is never invisible, it is just up to ten seconds late.
> **Refresh changes** in the header menu skips that wait and says what it found. The
> terminal UI has the same action as its `refresh-changes` command.

If a git operation collides with a lock, dux keeps retrying, so a single blip usually
clears itself. If it does not, you get a "Couldn't load changes" card with a Refresh button,
and a warning toast once the failures persist. A commit you make in the browser is an
ordinary commit, visible everywhere.

## When an agent deletes its own working copy

An agent works inside a git worktree, and it can remove that worktree from inside it:
merging its branch and then deleting the branch takes the directory with it. dux cannot
stop that, so it says so instead. The Changes pane goes quiet with a sentence naming the
directory that is gone, and the agent's row in the sidebar says **working copy missing**
where it would normally name the project.

> [!IMPORTANT]
> The agent's process, if it was still running, keeps running in a directory that no longer
> exists. Nothing it does from then on reaches your files. **Recreate working copy…** in the
> agent's `⋯` menu checks the agent's branch out again at the same path, which is what
> lets an agent whose CLI keeps its history per directory resume the same conversation. The
> code changes that were in the old directory are gone; this puts the directory back, not
> its contents. The terminal UI has the same action as its `recreate-working-copy` command.
>
> You can do it while the agent is running, and what happens next is the CLI's own answer.
> A running Claude tab never notices, and carries on in the recreated folder with its
> conversation intact. Every other CLI is the cautious case: Codex answers every turn with
> an invalid-directory error from the moment the folder goes and stays stuck after the
> recreate, so stop the tab and start the agent again to continue there. The confirmation
> says which of the two you are getting.

If the branch is gone locally but still on the remote, dux creates it again from
`origin/<branch>`, holding everything that had been pushed. If it is gone everywhere, dux
creates it again from the project's source branch, and the commits that branch held are not
coming back. The confirmation says all three, and the message afterwards names the one that
actually happened.

While the working copy is gone, the editor and a new terminal for that agent are refused
with the same sentence: there is no directory for either to open.
