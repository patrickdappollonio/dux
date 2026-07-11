---
title: The code editor
description: A real Monaco editor in the browser for any file in a worktree, with syntax highlighting, JSON and TOML help, Markdown preview, path search, diffs against HEAD, and open-in-local-editor.
group: Server mode
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

The overlay is two panes. On the left, a search box, a **New file** button, and a
file tree. On the right, the file itself: the editor, a diff view, or a Markdown
preview, depending on the toggles in the header. The header also carries the file
path, a dirty dot when you have unsaved edits, a read-only badge where it applies,
and **Save** and **Close**.

Save with the button or with `Ctrl+S` / `Cmd+S`. A toast confirms the write.

## Finding a file

The file tree lists the files in the worktree, with directories first and
alphabetical within a level, rendered as a virtualized list so even a large repo
scrolls smoothly. The ancestors of whatever file you have open expand
automatically. Changed files carry the same status icon you see in the [Changes
pane](/docs/web-git), so you can spot your edits at a glance. (The tree is a
plain filesystem walk of the worktree, not a git-aware listing: gitignored files
are included, and so is most of `.git/` itself. It is capped at 50,000 entries,
with a truncation notice if a repo has more. A filesystem-browser rework is in
progress, so treat this as the current, not final, behavior.)

The **Search files…** box does a fast, case-insensitive match on file paths across
the worktree, so you do not have to click through folders to reach a deeply nested
file. It matches paths, not file contents.

## Editing, creating, and saving

Any file inside the worktree is editable. Open it, type, and the dirty dot
appears; save writes it to disk. **New file** takes a worktree-relative path (its
parent folder must already exist), writes an empty file, and opens it ready to go.

The server keeps you inside the worktree and refuses to hand back things you
should not be editing: files outside the worktree, inside `.git`, or binary blobs
come back read-only or not at all, with a badge explaining why. If you try to
close or switch away with unsaved changes, dux asks first.

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
