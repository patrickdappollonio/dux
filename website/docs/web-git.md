---
title: Git without leaving the browser
description: The Changes pane in server mode, staged and unstaged groups, stage, unstage, discard with confirmation, commit, push, pull, forcing a refresh, the PR banner, and the shared file-status icons.
group: Web UI
order: 63
---

An agent changes files, and you want to see what it did before it goes anywhere.
The Changes pane in server mode is that review surface, and it does the everyday
git chores too, so you rarely need to drop to a shell for them.

## The Changes pane

The right-hand Changes pane tracks the focused agent's working tree. Files split
into two collapsible groups, **Staged** and **Unstaged**, each with a count badge,
and a filter box narrows a long list by path. Every row shows a status icon, the
file path, and the green and red line counts for the change. When the worktree is
clean it says so plainly.

Click any row to open its diff in the [code editor](/docs/web-editor), read-only
and syntax-highlighted, HEAD against the working copy.

## One icon for every status

Each file's git status is shown through a single shared status icon, the same one
the editor's file tree uses, so a modified file looks the same everywhere:

- **M** modified, **A** added, **D** deleted, **R** renamed, **C** copied,
  **U** conflict, **T** type changed, and **?** untracked.

Each carries a tooltip spelling the status out, so you are never guessing what a
glyph means.

## Stage, unstage, discard

Hover a file row for its `⋯` menu:

- **Stage** and **Unstage** move a file between the two groups. The row jumps to
  its new group as soon as the engine confirms the change.
- **Edit** opens the file in the editor (desktop only, and hidden for deleted
  files).
- **Discard…** throws away a file's uncommitted changes. It only shows up on
  **unstaged** rows, both in the menu and on the server: unstage a file first if
  you want to discard it. This one is destructive, so it always confirms first,
  and the dialog is honest about what it will do: an untracked file is
  **permanently deleted from disk**, a tracked file is **restored to its last
  committed state**. The server re-derives which case applies from live git
  status at the moment you confirm.

## Commit, push, pull

The pane header's `⋯` **Actions** menu carries the rest:

- **Commit…** opens a dialog for a multi-line message and commits **only the
  staged files**. It is disabled until something is staged, and `Cmd/Ctrl+Enter`
  submits.
- **Push** and **Pull** are one click each, with a progress toast that reports
  back to the browser tab you triggered them from.
- **Refresh changes** asks git again on the spot and reports what it found. dux
  has no file watcher, so a change dux did not make itself is only picked up by
  the next poll. This is the "I just did that, look again" button.
- **Hide Changes pane** tucks the whole pane away when you want the terminal full
  width. Bring it back from the **Show the Changes pane** row in **Preferences**:
  hiding the pane takes this menu with it.

## The PR banner

When a session is tied to a GitHub pull request, a one-line strip shows the PR
number, its state, and its title, color-coded (open is green, merged is purple,
closed is red), and clicking it opens the PR in a new tab. You can move the banner
above or below the terminal in **Preferences** (the cog menu).

There is no "create a PR" button in the web UI. The banner surfaces an **existing**
PR; agents open PRs themselves as part of their work. Pulling a PR's branch into a
fresh agent, on the other hand, is something the web UI does do, covered in
[Agents from the browser](/docs/web-agents).

## Staying in sync

There is no file watcher behind the Changes pane, and knowing that explains
everything it does. The pane updates the moment dux itself changes a file: a
stage, an unstage, a discard, a commit, a file saved in the editor. Everything
else is found by a background poll, an agent writing files in its worktree just
as much as a file you delete from a companion terminal, and a file you drop onto
a terminal is the one thing dux does that it does not notice this way. The poll
runs every couple of seconds while any agent or terminal in the workspace is
running, and every ten seconds while none is, so a change dux did not make is
never invisible, it is just up to ten seconds late.

**Refresh changes** in the header menu skips that wait and says what it found.
The terminal UI has the same action as its `refresh-changes` command.

If a git operation collides with a lock, the background poller keeps retrying on
its own, so a single blip usually clears itself before you notice. If it does
not, dux shows a "Couldn't load changes" card with a Refresh button, and a
warning toast fires once the failures persist across several attempts, so you
are never left guessing why the pane went quiet. All of this rides the same
engine and the same worktrees the terminal UI uses, so a commit you make in the
browser is simply a commit, visible everywhere.
