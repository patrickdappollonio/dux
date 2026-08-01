---
title: The code editor
description: A real Monaco editor in the browser for any file in a worktree, with syntax highlighting, JSON and TOML help, Markdown preview, path search, diffs against HEAD, and open-in-local-editor.
group: Web UI
order: 62
---

Sometimes you do not want to ask the agent to fix a typo, you just want to fix it.
Server mode ships a real code editor, built on **Monaco** (the engine behind VS
Code), right in the page. No round trip to a separate app, no leaving the
workspace. Open a file, edit it, save it, and the agent working in that worktree
sees your change on disk immediately.

## Opening it

The editor opens as a full-screen overlay from a few places:

- An agent's `⋯` menu has **Open editor**.
- A changed file's `⋯` menu has **Edit**, and clicking a changed file opens its
  diff (more on that below).

It loads only when you open it, so it costs nothing until you use it.

The editor is **desktop-only.** Monaco is a poor experience on a touch screen, so
on a phone the editor overlay does not open. You can still browse your changed
files on the mobile Changes screen, but reviewing a full diff and editing wait for
a real keyboard.

## The layout

The overlay is two panes. On the left, a search box, a header shortcut to create a
file at the worktree root, and a file tree. On the right, the file itself: the
editor, a diff view, or a Markdown preview, depending on the toggles in the header.
The header also carries the file path, a dirty dot when you have unsaved edits, a
read-only badge where it applies, and **Save** and **Close**.

Save with the button or with `Ctrl+S` / `Cmd+S`. A toast confirms the write.

## Tabs

Open more than one file and the editor grows a tab strip, the way any real
editor does, so you can flip between several open files without losing your
place in each one.

Single-clicking a file in the tree opens it in a **preview tab**, shown in
italic. A preview tab is reusable: click another file and it replaces the same
tab instead of piling up a new one, so browsing around the tree to find the
right file does not leave a trail of tabs behind you. The moment you actually
edit the file, or double-click it in the tree (or double-click its tab), it is
promoted to a permanent tab, and a new preview tab is free to take over for the
next file you are just looking at.

You can have as many permanent tabs open as you like, each with its own edit
history, scroll position, and cursor location, so switching back to a tab
returns you to exactly where you left it. Close a tab with its `×` control or a
middle-click; if it has unsaved changes, dux asks before discarding them.

## Finding a file

The file tree is a lazy filesystem browser: it loads one directory at a time, as
you expand it, rather than walking the whole worktree up front. That means it
stays fast and effectively uncapped no matter how large the repo is, and a huge
sibling folder you never open costs nothing. Directories list before files,
alphabetically within each level, and the whole tree renders as a virtualized
list so scrolling stays smooth even with a folder expanded wide. The ancestors of
whatever file you have open expand automatically. Changed files carry the same
status icon you see in the [Changes pane](/docs/web-git), so you can spot your
edits at a glance.

The tree is a plain filesystem listing, not a git-aware one: gitignored files are
included, and so is `.git/` itself, browsable like any other folder. Anything
inside `.git/` opens read-only, so you can look but not edit it.

Each row also carries a file-type icon on the left, monochrome so it never
competes with the git-status marker on the right: folders look different open,
closed, or empty, and files get an icon for their kind (code, image, config,
Markdown, lockfile, binary, and a generic fallback for everything else). It is a
glance-level cue, not a replacement for opening the file.

The **Search files…** box does a fast, case-insensitive match on file paths across
the worktree, so you do not have to click through folders to reach a deeply nested
file. It matches paths, not file contents. Unlike the tree, search works off a
flat index built by walking the worktree once, and that index is capped (50,000
files by default, configurable via `[server] search_index_max_files`, `0`
disables the cap) so a very large repository still gets a bounded, fast search
rather than an unbounded walk. The tree itself has no such cap: it only ever asks
for one directory at a time.

## Editing and saving

Any file inside the worktree is editable. Open it, type, and the dirty dot
appears; save writes it to disk.

The server keeps you inside the worktree and refuses to hand back things you
should not be editing: files outside the worktree, inside `.git`, or binary blobs
come back read-only or not at all, with a badge explaining why. If you try to
close or switch away with unsaved changes, dux asks first.

## Creating, renaming, and deleting files

Right-click anywhere in the file tree to manage files and folders without leaving
the editor:

- **New File…** and **New Folder…** are always on the menu. Right-click a file row
  and the new entry lands in that file's own folder; right-click a folder row and
  it lands inside that folder; right-click the empty area below the tree and it
  lands at the worktree root. A brand-new file opens immediately, ready to type
  into.
- **Rename…** works on files and folders alike. If the file has unsaved changes,
  dux blocks the rename until you save or discard them first, rather than
  silently reloading your edits away.
- **Delete…** is permanent. There is no trash on the server: confirming deletes
  the file, or the folder and everything inside it, straight from disk. If the
  file you deleted was open, its tab closes along with it.

Renaming or deleting a file that has other open editor tabs pointed at it (or, for
a folder, tabs pointed anywhere underneath it) keeps everything in sync: a rename
retargets those tabs to the new path, and a delete closes them.

## Syntax highlighting and language niceties

Around eighty languages get proper syntax highlighting out of the box: Rust, Go,
Python, JavaScript and TypeScript, C and C++, Java, shell, SQL, YAML, HCL,
Dockerfile, Markdown, and many more, including bare-filename matches like
`Makefile` and `Dockerfile`. Two file types get extra help beyond coloring:

- **JSON** gets full validation, so a misplaced comma is flagged as you type. This
  is the same editor that powers dux's own **Edit config** palette command.
- **TOML** gets a dedicated tokenizer, which is exactly what you want when you are
  editing a `config.toml`.

This is a deliberately trimmed Monaco: highlighting and JSON validation, but no
heavyweight IntelliSense or cross-file diagnostics. It is a fast, honest text
editor with great highlighting, not a full IDE.

## Markdown preview

For Markdown files (`.md`, `.markdown`, and friends) a **Preview / Edit** toggle
renders the current buffer, unsaved edits included, so you can check how a README
reads without saving first. It handles GitHub-flavored Markdown, hides a YAML
frontmatter block, and rewrites relative image paths so they load from the
worktree.

## Diffs against HEAD

Flip to the **Diff** view (or click a changed file in the Changes pane) to see a
read-only, syntax-highlighted comparison of the file's working copy against its
last committed version, shown inline. Added files render as all-insert, deleted
files as all-delete. If the file changes on disk while you are looking, dux does
not yank the diff out from under you: an amber **Reload** button appears so you
refresh on your own terms.

## Open in your local editor

Prefer your own editor? The **Open editor ▾** dropdown launches **Cursor, VS
Code, Zed, VSCodium, or Sublime Text** on the file. Because it spawns that editor
**on the server** (the machine dux is running on), it only makes sense when you
are sitting at that machine. dux enables it only for local-access URLs (loopback,
`0.0.0.0`, or a private LAN address) and disables it with a tooltip when you have
reached dux over a remote URL, where launching a GUI editor on the server would do
you no good at all.
