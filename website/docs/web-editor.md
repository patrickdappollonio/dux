---
title: The code editor
description: A real Monaco editor in the browser for any file in a worktree or a terminal's directory, with previews, path search, diffs against HEAD, file management, drag-in uploads, and open-in-local-editor.
group: Web UI
order: 62
---

Sometimes you do not want to ask the agent to fix a typo, you just want to fix it. Server
mode ships a real code editor, built on **Monaco** (the engine behind VS Code), right in
the page. Open a file, edit it, save it, and the agent working in that worktree sees your
change on disk immediately.

## Opening it

The editor opens as a full-screen overlay from a few places:

- An agent's `⋯` menu has **Open editor here**.
- A terminal's `⋯` menu has the same pair, rooted at the directory that terminal opened
  in (see below).
- A changed file's `⋯` menu has **Edit**, and clicking a changed file opens its diff.

For a separate browser tab, the agent's `⋯` menu also has **Open editor in new tab**, and
the editor's header carries a matching icon that opens the current file there
(middle-click works, it is a real link). That tab is nothing but the editor,
full-viewport, named at the top by whatever it is rooted at. There is no in-app link back
to the workspace: your browser's Back button or closing the tab is the way out.

The standalone tab is deliberately quiet: workspace messages and the "needs you" count in
the tab title belong to the workspace tab. What the editor itself does still tells you
there, so saves, renames, deletes and uploads confirm in the tab you did them in.

> [!IMPORTANT]
> The editor overlay is **desktop-only**. Monaco is a poor experience on a touch screen,
> so on a phone the overlay does not open. The standalone tab is the deliberate exception.

The standalone tab works on phones, best-effort. The file explorer starts collapsed so the
file gets the width (the header's toggle reopens it at a phone-sized width), and the
editor stays above the soft keyboard. Only the explorer toggle and **Save** sit inline;
the File/Diff switch, the preview toggle and **Open local editor** fold into one `⋯` menu
at the end of the row.

## Editing next to a terminal

A terminal's two entries open the editor rooted at the directory that terminal started in.
A project terminal gets the repo root, a standalone terminal gets wherever it opened, home
included. It is the whole tree from there, files you can create and rename and save.

The root is pinned to where the terminal **started** and stays there when you `cd`, so
your file tree, open buffers, unsaved drafts and bookmarkable address keep meaning the
same thing.

A terminal spawned by an agent lives in that agent's worktree, so its entries open the
agent's editor, with the full git surface.

A terminal-rooted editor has no **Diff** view (a plain directory has no last-committed
version to compare against) and no live changed-files updates. Everything else is the same
editor, and freshness still works off the other two signals: come back to the tab, or
click into it, and dux re-checks what is open.

> [!WARNING]
> An editor never outlives what it is rooted at. Close the terminal and its editor goes
> with it, saying so rather than blanking. If you had unsaved text in it, dux stops and
> asks first, and keeps the words on screen so you can copy them out. There is nowhere left
> to save them to.

## The URL knows where you are

The address bar names the editor and the file you are looking at, so a hard refresh
reopens the editor on the same file and view, and a bookmark or shared link lands there
directly. Opening the editor is exactly one step in your browser history, and switching
files inside it updates the address in place, so one press of Back closes the editor and
returns you to whatever was behind it. Closing it that way loses nothing (see below).

## The layout

The overlay is two panes. On the left, a search box, a header shortcut to create a file at
the worktree root, and a file tree. On the right, the file itself: the editor, a diff
view, or a rendered preview, depending on the header toggles. The header also carries the
file path, a dirty dot for unsaved edits, a read-only badge where it applies, and **Save**
and **Close**.

Drag the divider to resize the explorer pane, or collapse it with the toggle at the left
end of the header. Your width (in pixels) and collapsed state are remembered in the
browser, so the editor reopens the way you left it in the overlay and in its own tab
alike.

Save with the button or with `Ctrl+S` / `Cmd+S`. A toast confirms the write, and creating,
renaming, moving and deleting each confirm with a toast naming what happened, or say
plainly when something went wrong.

The header also shows the file's syntax language as a dropdown. dux lets Monaco guess from
the file name, and when the guess is wrong (a `Cargo.lock` that is really TOML, a config
file with no extension) you can pick the right one from the full list. The choice applies
to the open file including its diff view, follows a rename or move, and lasts until you
close the file. **Auto** at the top of the list hands the decision back to Monaco.

## Tabs

Open more than one file and the editor grows a tab strip.

Single-clicking a file in the tree opens it in a **preview tab**, shown in italic. A
preview tab is reusable: click another file and it replaces the same tab, so browsing the
tree does not leave a trail behind you. Editing the file, double-clicking it in the tree,
or double-clicking its tab promotes it to a permanent tab, and a new preview tab is free to
take over.

You can have as many permanent tabs as you like, each with its own edit history, scroll
position and cursor location. Close a tab with its `×` control or a middle-click; if it has
unsaved changes, dux asks before discarding them.

## Finding a file

The file tree is a lazy filesystem browser: it loads one directory at a time as you expand
it, so it is effectively uncapped no matter how large the repo is. Directories list before
files, alphabetically within each level, and the tree is virtualized so scrolling stays
smooth. The ancestors of the file you have open expand automatically. Changed files carry
the same status icon as the [Changes pane](/docs/web-git).

> [!NOTE]
> The tree is a plain filesystem listing, not a git-aware one. Gitignored files are
> included, and so is `.git/` itself, browsable like any other folder. Anything inside
> `.git/` opens read-only.

Each row carries a monochrome file-type icon on the left, so it never competes with the
git-status marker on the right: folders differ open, closed and empty, and files get an
icon for their kind (code, image, config, Markdown, lockfile, binary, and a generic
fallback).

The **Search files…** box does a fast, case-insensitive match on file **paths**, not
contents. It is capped at 50,000 files by default, configurable via
`[server] search_index_max_files`, with `0` disabling the cap. The tree itself has no such
cap.

## Editing and saving

Any file inside the worktree is editable. Open it, type, and the dirty dot appears; save
writes it to disk.

dux keeps you inside the worktree: files outside it, inside `.git`, or binary blobs come
back read-only or not at all, with a badge explaining why.

Unsaved edits survive the editor closing. Close it with the button, Escape, or the
browser's Back, and reopening brings every tab back, dirty dot and typed text included. The
one real discard is closing a dirty **tab**, which still asks first.

> [!WARNING]
> Drafts live in the page, so a hard refresh or closing the browser tab loses them. The
> browser asks before leaving while any draft is unsaved, even if the editor is closed at
> the time. Save it or discard its tab and the prompt stops. The one silent exception: when
> dux itself restarts the page reloads without asking, and in-page drafts do not survive.

Two size limits, both generous:

- A file over **5 MiB** does not open in the editor at all; you get the read-only badge
  saying so.
- A save is refused past roughly **10 MiB**. The refusal names the limit and **your text
  is kept**: the tab stays dirty with everything you typed, so you can trim it down and
  save again, or copy it out.

## When the agent edits a file you have open

**If you have not touched the file**, it refreshes in place, with no banner and nothing to
click, and your scroll position and undo history come along.

**If you have unsaved edits**, a notice appears across the top of the pane saying the file
changed on disk, offering **Reload from disk** (which confirms first, because it discards
everything you typed) and **Keep mine**. If the file was deleted rather than changed, the
notice says so and offers to close the tab or keep your copy open.

**Your save cannot clobber the agent's work.** dux refuses a save that would overwrite a
file changed since you opened it, and gives you three choices: overwrite anyway, reload the
disk version, or cancel. Cancelling keeps your text exactly as typed.

> [!NOTE]
> dux does not watch the filesystem. It checks when git reports the file as changed, when
> you come back to the browser window, and when you switch to the tab. The one gap is a
> file that changes while you sit on its tab with the window already focused and git
> silent, and looking away and back resolves it.

## Creating, renaming, moving, and deleting files

Right-click anywhere in the file tree:

- **New File…** and **New Folder…** are always on the menu. Right-click a file row and the
  new entry lands in that file's folder; right-click a folder row and it lands inside it;
  right-click the empty area below the tree and it lands at the worktree root. A brand-new
  file opens immediately.
- **Upload here…** takes a file off the computer you are sitting at and puts it in the
  folder you right-clicked, resolved exactly as New File would. It pastes nothing into any
  terminal (see [Dropping and pasting files](/docs/dropping-files)) and is absent when the
  server has uploads switched off.
- **Rename…** works on files and folders alike. If the file has unsaved changes, dux blocks
  the rename until you save or discard them.
- **Move…** puts the entry in a different folder, name unchanged. You get a folder browser
  opening on the folder the entry is already in, and the line above it always spells out
  exactly where the thing will land. Same unsaved-changes rule as Rename, and the same tab
  bookkeeping.
- **Delete…** is permanent.
- **File info…** (**Folder info…** on a folder) is read-only: full path; whether it is a
  file, a folder or a symlink (and for a symlink, what it points at); size in human units
  and exact bytes; last modified in your own timezone; the permission bits both as
  `rw-r--r--` and as `644`; and what git makes of it. Git gets its own answer rather than a
  guess: unmodified, the exact change and which side it is on, ignored, inside a different
  repository (a nested clone or a submodule), or no repository at all. Ignored and nested
  are named explicitly because `git status` says nothing about either.

  The panel reads the file when it opens and again whenever you come back to the browser
  tab, and that is all: there is no polling. Delete the file elsewhere and switch back and
  the panel closes itself rather than describing something that is not there. If it
  vanishes while you sit on the focused tab, the panel will not know until the tab regains
  focus.

> [!CAUTION]
> **There is no trash on the server.** Confirming a delete removes the file, or the folder
> and everything inside it, straight from disk, and an open tab on it closes.
>
> For the same reason, a **Move…** onto a name that is already taken is **refused
> outright**, not offered as an "are you sure?". Rename one of the two first, then move.

Renaming, moving, or deleting a file that has other open editor tabs pointed at it (or, for
a folder, tabs pointed anywhere underneath it) keeps everything in sync: a rename or move
retargets those tabs, and a delete closes them.

## Dragging files in from your desktop

Drag files from your own machine onto the file tree and let go, and they are saved into the
worktree on the server. This is how you get a logo, a fixture, a CSV or a screenshot **into
the project** from a laptop that is not the machine dux runs on.

Where they land is where you point:

- **A folder row** takes the files into that folder.
- **A file row** takes them into the folder that file is in, the same rule New File…
  follows.
- **The empty space** below the tree takes them to the worktree root.

The receiving row is highlighted while you are still holding the files, and a toast
afterwards names the folder.

These are **ordinary files**: `git status` shows them, they appear in the agent's Changes
pane straight away, and you can commit them. That is the difference between this and
dropping a file onto an agent's pane, which is scratch that dies with the agent. See
[Dropping and pasting files](/docs/dropping-files).

A drop here pastes nothing into any terminal and needs no input ownership: it saves files
and stops.

Nothing on disk is overwritten. If a name is taken the file is saved under a new one, with
a timestamp and a counter, and the toast tells you what it is called now. (**Move…** above
instead refuses an occupied destination outright, because a move names one exact
destination while a drop names only a folder. Neither overwrites.) Your filenames are kept
exactly as you had them, accents and all, and a name dux cannot use is refused with the
reason rather than rewritten.

Dropping outside the worktree or into `.git` is refused, exactly as creating, renaming or
moving into either is.

> [!IMPORTANT]
> A drop is **stricter than New File…, Rename… and Move… in one way**: those three follow a
> symlinked directory that stays inside the worktree, so `New File…` inside a
> `libs -> packages/libs` link works, while a drop refuses any folder reached through a link
> and says so. To drop into a linked folder, navigate to the real one.

The tree and the file search pick them up as soon as they land.

dux takes **files**, not folders. Dropping a folder is refused by name, with the files
dropped alongside it still saved, and one toast says which was which. A drop that misses the
tree entirely, landing on the editor or the tab strip, does nothing at all, rather than the
browser navigating the tab away and taking everything unsaved with it.

The whole feature is off when `[server] file_drop_max_bytes` is `0` (the tree simply does not
react to a drag), and the same size limit applies here as anywhere else.

## Syntax highlighting and language niceties

Around eighty languages get proper syntax highlighting out of the box: Rust, Go, Python,
JavaScript and TypeScript, C and C++, Java, shell, SQL, YAML, HCL, Dockerfile, Markdown and
many more, including bare-filename matches like `Makefile` and `Dockerfile`. Two file types
get extra help:

- **JSON** gets full validation, so a misplaced comma is flagged as you type. This is the
  same editor behind the app menu's **Edit config file…** entry.
- **TOML** gets a dedicated tokenizer, which is what you want when editing a `config.toml`.

This is a deliberately trimmed Monaco: highlighting and JSON validation, but no
IntelliSense or cross-file diagnostics.

## Markdown and SVG preview

For Markdown files (`.md`, `.markdown`, and friends) a **Preview / Edit** toggle renders the
current buffer, unsaved edits included. It handles GitHub-flavored Markdown, hides a YAML
frontmatter block, and rewrites relative image paths so they load from the worktree.

SVG files get the same treatment: they open as text and the toggle renders the drawing from
whatever is in the buffer right now.

The toggle is there in the **Diff** view too, so a changed README can be read as a rendered
page without flipping to File first. It always shows the file's end state: your unsaved
draft when you have one, otherwise the file on disk. Toggling it off puts you back on the
diff.

## Images

Image files (PNG, JPEG, GIF, WebP, and the rest) skip the editor and open as the picture
itself, centered on the right, with the path and pixel dimensions underneath. There is
nothing to save, no preview toggle and no diff view: clicking a changed image in the Changes
pane shows the picture as it is on disk right now.

## Diffs against HEAD

This one is for agent worktrees; a terminal-rooted editor has no Diff button.

Flip to the **Diff** view (or click a changed file in the Changes pane) to see a read-only,
syntax-highlighted comparison of the file's working copy against its last committed version,
shown inline. Added files render as all-insert, deleted files as all-delete. If the file
changes on disk while you are looking, an amber **Reload** button appears so you refresh on
your own terms.

## Open in your local editor

The **Open local editor ▾** dropdown launches **Cursor, VS Code, Zed, VSCodium, or Sublime
Text** on the file.

> [!IMPORTANT]
> It spawns that editor **on the server** (the machine dux runs on), so it only makes sense
> when you are sitting at that machine. dux enables it only for local-access URLs (loopback,
> `0.0.0.0`, or a private LAN address) and disables it with a tooltip when you reached dux
> over a remote URL.
