---
title: The code editor
description: A real Monaco editor in the browser for any file in a worktree, with syntax highlighting, JSON and TOML help, Markdown and SVG previews, image viewing, path search, diffs against HEAD, file management including moving and inspecting files, dragging files in from your desktop, and open-in-local-editor.
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

- An agent's `⋯` menu has **Open editor here**.
- A changed file's `⋯` menu has **Edit**, and clicking a changed file opens its
  diff (more on that below).

It loads only when you open it, so it costs nothing until you use it.

Prefer the editor in its own browser tab? The agent's `⋯` menu also has **Open
editor in new tab**, and the editor's own header carries a matching icon that
opens the current file in that standalone tab (middle-click works, it is a real
link). The standalone tab is nothing but the editor, full-viewport, with the
agent's name at the top. There is no in-app link back to the workspace — the
tab is yours, so your browser's Back button or closing the tab is the way out.

The editor overlay is **desktop-only**: Monaco is a poor experience on a touch
screen, so on a phone the overlay does not open. The standalone tab is the
deliberate exception. It works on phones, best-effort, with the file explorer
starting collapsed so the file itself gets the width (the header's toggle
reopens it at a phone-sized width), and it keeps the editor above the soft
keyboard. The phone header stays lean, too: only the explorer toggle and
**Save** sit inline, and everything else — the File/Diff switch, the preview
toggle, **Open local editor** — folds into one `⋯` menu at the end of the row.
Fixing a typo from the couch is exactly what it is for; long editing sessions
still want a real keyboard.

## The URL knows where you are

The address bar names the editor and the file you are looking at, so the
position survives anything a URL survives: a hard refresh reopens the editor on
the same file (and view), a bookmark or a shared link lands there directly, and
the standalone tab is just another address. Opening the editor is exactly one
step in your browser history, wherever you opened it from, and switching files
inside it updates the address in place rather than piling up entries — so one
press of Back closes the editor and returns you to whatever you were looking
at before it opened. Closing it that way loses nothing — see below.

## The layout

The overlay is two panes. On the left, a search box, a header shortcut to create a
file at the worktree root, and a file tree. On the right, the file itself: the
editor, a diff view, or a rendered preview, depending on the toggles in the header.
The header also carries the file path, a dirty dot when you have unsaved edits, a
read-only badge where it applies, and **Save** and **Close**.

The explorer pane is yours to shape: drag the divider to resize it, or collapse it
entirely with the toggle at the left end of the header when you want the whole
width for the file. Your width and collapsed/expanded choice are remembered in the
browser, so the editor reopens the way you left it. The width is remembered in
pixels, on purpose: whether the editor is the overlay or its own browser tab, the
tree renders at the same width instead of growing with the window.

Save with the button or with `Ctrl+S` / `Cmd+S`. A toast confirms the write, and
the same goes for the file operations: creating, renaming, moving, and deleting a
file or folder each confirm with a toast naming what happened, or tell you plainly
when something went wrong.

The header also shows the file's syntax language, and it is a dropdown: dux lets
Monaco guess the language from the file name, and when the guess is wrong (a
`Cargo.lock` that is really TOML, a config file with no extension) you can pick
the right one from the full list. The choice applies to the open file, including
its diff view, and lasts until you close it; **Auto** at the top of the list hands
the decision back to Monaco.

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
come back read-only or not at all, with a badge explaining why.

Unsaved edits survive the editor closing. Close it — the button, Escape, the
browser's Back — and your drafts stay put: reopen the editor and every tab is
back, dirty dot and typed text included. The one real discard is closing a
dirty **tab**, which still asks first. Because drafts live in the page, a hard
refresh or closing the browser tab really would lose them, so the browser asks
before leaving while any draft is unsaved — even if the editor itself is
closed at the time. Deal with the draft (save it, or discard its tab) and the
prompt stops. The one silent exception: when dux itself restarts, the page
reloads without asking, and in-page drafts do not survive that.

There are two size limits, and they are generous enough that you are unlikely to
meet either. A file over **5 MiB** does not open in the editor at all; you get the
read-only badge saying so. A save is refused past roughly **10 MiB**, measured on
the request rather than the file, so reaching it means having more than doubled
the file's size in one sitting. If that happens, the refusal says what the limit
is and **your text is kept**: the tab stays dirty with everything you typed still
in it, so you can trim it down and save again, or copy it out. Nothing is
discarded.

## Creating, renaming, moving, and deleting files

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
- **Move…** puts the entry in a different folder, name unchanged. There is no
  cut and paste to remember: you get a little folder browser instead, opening on
  the folder the entry is already in, one click to step into a subfolder and one
  to climb back out. The line above the browser always spells out exactly where
  the thing will land, so nothing hinges on you reading a breadcrumb correctly.
  Same unsaved-changes rule as Rename, and the same tab bookkeeping. A move that
  would land on a name that is already taken is **refused outright**, not offered
  as an "are you sure?": there is no trash on the server, so an overwrite here
  would destroy the file that was already there with nothing to undo it. Rename
  one of the two first, then move.
- **Delete…** is permanent. There is no trash on the server: confirming deletes
  the file, or the folder and everything inside it, straight from disk. If the
  file you deleted was open, its tab closes along with it.
- **File info…** (**Folder info…** on a folder) is the read-only one: full path,
  whether it is a file, a folder or a symlink (and for a symlink, what it points
  at), size in both human units and exact bytes, when it was last modified in
  your own timezone, the permission bits both as `rw-r--r--` and as `644`, and
  what git currently makes of it. Git gets its own answer rather than a guess:
  unmodified, the exact change and which side it is on, ignored, inside a
  different repository (a nested clone or a submodule), or no repository at all.
  Ignored and nested get named explicitly because `git status` says nothing
  whatsoever about either, so anything less would report your `node_modules` as
  a tracked, unmodified file.

  The panel reads the file once when it opens and again whenever you come back
  to the browser tab, and that is all: there is no polling. So if you delete the
  file somewhere else and switch back, the panel notices it is gone and closes
  itself instead of describing something that is not there. If it vanishes while
  you are sitting on this tab watching it, the panel will not know until the tab
  regains focus.

Renaming, moving, or deleting a file that has other open editor tabs pointed at it
(or, for a folder, tabs pointed anywhere underneath it) keeps everything in sync: a
rename or a move retargets those tabs to the new path, and a delete closes them.

## Dragging files in from your desktop

Drag files from your own machine onto the file tree and let go, and they are
saved into the worktree on the server. This is how you get a logo, a fixture, a
CSV or a screenshot **into the project** from a laptop that is not the machine
dux is running on.

Where they land is where you point:

- **A folder row** takes the files into that folder.
- **A file row** takes them into the folder that file is in, because a file is
  not a place to put a file. It is the same rule New File… follows.
- **The empty space** below the tree takes them to the worktree root.

The row that would receive the drop is highlighted while you are still holding
the files, so you can see the destination before you commit to it, and a toast
afterwards names the folder they went to.

These are **ordinary files**. They show up in `git status`, they show up in the
agent's Changes pane straight away, and you can commit them. Nothing is hidden
and no ignore file is written. That is deliberate, and it is the difference
between this and dropping a file onto an agent's pane: a file handed to an agent
is scratch that dies with the agent, and a file dropped here is one you are
adding to the repository. dux calls those two different intents and gives them
different destinations. See
[Dropping and pasting files](/docs/dropping-files) for the whole picture.

A drop here pastes nothing into any terminal, and it does not need you to hold
input on anything: it saves files and stops.

Nothing already on disk is overwritten. If a name is taken the file is saved
under a new one, with a timestamp and a counter, and the toast tells you what it
is called now. (Note the deliberate difference from **Move…** above, which
refuses an occupied destination outright. A move names one exact destination, so
putting the file somewhere else would be a different operation from the one you
asked for; a drop names only a folder, so the next free name is the honest
answer. Neither overwrites.) Your filenames are kept exactly as you had them,
accents and all, and a name dux cannot use is refused with the reason rather
than rewritten.

Dropping outside the worktree or into `.git` is refused, exactly as creating,
renaming or moving into either is. A drop is **stricter than those three in one
way**, and it is worth knowing about rather than being surprised by: **New
File…**, **Rename…** and **Move…** will follow a symlinked directory that stays
inside the worktree, so `New File…` inside a `libs -> packages/libs` link works,
while a drop refuses any folder reached through a link and says so. That is the
safe direction to be wrong in (a link is the one way a perfectly ordinary
relative path still lands outside the tree), so it stays; if you want to drop
into a linked folder, navigate to the real one and drop there. Both the tree and
the file search index refresh as soon as the files land.

dux takes **files**, not folders. Dropping a folder is refused by name, with the
files dropped alongside it still saved, and one toast says which was which. And
a drop that misses the tree entirely, landing on the editor or the tab strip, is
swallowed rather than handled by the browser, which would otherwise navigate the
tab to the file you dropped and throw away everything you had not saved.

The whole feature is switched off when `[server] file_drop_max_bytes` is `0`,
and the same size limit applies here as anywhere else; the tree simply does not
respond to a drag when it is off.

## Syntax highlighting and language niceties

Around eighty languages get proper syntax highlighting out of the box: Rust, Go,
Python, JavaScript and TypeScript, C and C++, Java, shell, SQL, YAML, HCL,
Dockerfile, Markdown, and many more, including bare-filename matches like
`Makefile` and `Dockerfile`. Two file types get extra help beyond coloring:

- **JSON** gets full validation, so a misplaced comma is flagged as you type. This
  is the same editor behind the app menu's **Edit config file…** entry.
- **TOML** gets a dedicated tokenizer, which is exactly what you want when you are
  editing a `config.toml`.

This is a deliberately trimmed Monaco: highlighting and JSON validation, but no
heavyweight IntelliSense or cross-file diagnostics. It is a fast, honest text
editor with great highlighting, not a full IDE.

## Markdown and SVG preview

For Markdown files (`.md`, `.markdown`, and friends) a **Preview / Edit** toggle
renders the current buffer, unsaved edits included, so you can check how a README
reads without saving first. It handles GitHub-flavored Markdown, hides a YAML
frontmatter block, and rewrites relative image paths so they load from the
worktree.

SVG files get the same treatment: they open in the editor as text, and the same
toggle renders the drawing from whatever is in the buffer right now, saved or not,
so you can tweak a path and see the shape move before committing to it.

The toggle is there in the **Diff** view too, so a changed README clicked in the
Changes pane can be read as a rendered page without flipping the tab to File
first. It always shows the file's end state: your unsaved draft when you have
one, otherwise the file as it is on disk. Toggling it off puts you right back on
the diff.

## Images

Image files (PNG, JPEG, GIF, WebP, and the rest) are not text, so they skip the
editor entirely and open as the picture itself, centered on the right, with the
path and pixel dimensions underneath. There is nothing to save, no preview
toggle to press, and no diff view either: clicking a changed image in the
Changes pane shows you the picture as it is on disk right now, which is what
you wanted to see anyway. It is simply the fastest way to check what an agent
just drew into the worktree.

## Diffs against HEAD

Flip to the **Diff** view (or click a changed file in the Changes pane) to see a
read-only, syntax-highlighted comparison of the file's working copy against its
last committed version, shown inline. Added files render as all-insert, deleted
files as all-delete. If the file changes on disk while you are looking, dux does
not yank the diff out from under you: an amber **Reload** button appears so you
refresh on your own terms.

## Open in your local editor

Prefer your own editor? The **Open local editor ▾** dropdown launches **Cursor, VS
Code, Zed, VSCodium, or Sublime Text** on the file. Because it spawns that editor
**on the server** (the machine dux is running on), it only makes sense when you
are sitting at that machine. dux enables it only for local-access URLs (loopback,
`0.0.0.0`, or a private LAN address) and disables it with a tooltip when you have
reached dux over a remote URL, where launching a GUI editor on the server would do
you no good at all.
