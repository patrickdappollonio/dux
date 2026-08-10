---
title: Dropping and pasting files onto an agent
description: Drag a screenshot from your desktop onto an agent, a terminal or the editor's file tree in the browser, or just press paste, and dux puts it where you meant it. Where files land, why an agent's uploads stay out of git, why an editor drop is an ordinary committable file, why nothing is ever overwritten, and why this is web-only.
group: Web UI
order: 67
---

You have a screenshot. Your agent is running on a machine somewhere else, and
you are looking at it through a browser tab. Getting that image in front of the
agent used to mean copying it to the server by hand and typing out the path.

Now you drag it onto the terminal and let go. dux saves the file on the server
and pastes its path into the prompt, ready for you to finish the sentence.

## Or just paste it

You do not have to find the file first. Take the screenshot, press
`Ctrl+v` (`Cmd+v` on a Mac), and the image on your clipboard takes exactly the
same route: saved on the server, path pasted into the prompt. It is the shorter
gesture and it is the one most people reach for, because a screenshot is already
on the clipboard the moment you take it.

Everything below applies to a paste as much as to a drop: the same folders, the
same ignore file, the same collision-safe naming, the same per-provider path
shape, the same one toast at the end.

A few specifics worth knowing:

- **Images only.** A clipboard carrying an image is taken over; anything else,
  text included, is left completely alone and pastes exactly as it always has.
  When the clipboard carries *both* (copying a screenshot out of an application
  routinely does), the image wins and the text is not pasted, because pasting
  both would drop the markup into your prompt beside the path.
- **`Ctrl+Shift+v` forces the text.** Image-wins is the right default for a
  screenshot and the wrong one for rich content: copy a range of spreadsheet
  cells and the clipboard carries a picture of the cells alongside the numbers,
  and you almost certainly wanted the numbers. `Ctrl+Shift+v` (`Cmd+Shift+v` on
  a Mac) skips image handling entirely and pastes the text, exactly as that
  chord does elsewhere. It applies to that one keystroke; the next `Ctrl+v` is
  image-wins again.
- **On a phone, the path joins your draft.** With the compose bar up, a pasted
  image puts its path into the message you are composing rather than sending
  anything. You finish the sentence and press Send, as usual. The toast says so
  too, rather than claiming the agent already has it.
- **A screenshot usually has no name of its own.** Browsers hand one over as
  `image.png`, so several pastes collide, and dux does what it does for any
  collision: saves the next one under a new name and tells you what that name
  is. When the clipboard supplies no name at all, dux invents one from the
  clock, like `pasted-2026-08-09-141530.png`.

### Why paste and not "read my clipboard"

Browsers have an API for reading the clipboard on demand, and dux deliberately
does not use it. It is blocked outside a **secure context**, and dux is routinely
served over plain HTTP on a Tailscale address, which is exactly the deployment
this feature is for. The `paste` event has no such requirement, because your
keystroke *is* the permission: the browser hands the bytes to the page precisely
because you asked it to. That is the same reasoning behind `Ctrl+v` being the
reliable paste chord in the web terminal (see
[The workspace in the browser](/docs/web-workspace)).

## Why a path and not the file itself

The obvious idea is to shove the file's bytes into the terminal. That cannot
work, and it is worth knowing why, because it explains the whole design.

No agent CLI reads a file out of its input stream. They take a **path**, or they
read the **clipboard of the machine they are running on**, which when you are
driving from a browser is entirely the wrong computer. Claude Code shells out to
`xclip` or `wl-paste`; Codex takes `--image <FILE>` and also accepts a pasted
path; OpenCode's own docs say dragging works in "a terminal that exposes the
dropped file path to the TUI".

That is also what every terminal emulator does. Alacritty, kitty, WezTerm,
Ghostty, GNOME Terminal, Konsole and iTerm2 all answer a drop by typing the
path in for you. None of them sends contents. dux does the same thing, with the
one extra step a browser needs: it puts the file on the server first, since that
is where the agent can actually see it.

## Two drops, two intents

Dropping a file means one of two things, and dux stopped pretending it was one.

Handing an image to an agent is **"look at this for me"**. It is scratch: you
want the agent to read it, and then you want it gone. Dropping a file into a
terminal, or onto the editor's file tree, is **"put this here"**. It is a file
you are placing in a folder you chose, and you meant it.

Those want opposite things from git, so they get different destinations. Two
intents, three places you can drop: the agent's pane, a terminal, and the
editor's file tree.

## Where the file lands

**On an agent's pane**, in that agent's upload folder: `.dux/uploads` inside the
agent's worktree, created the first time you drop something. It is **invisible
to git**, and it is **deleted with the agent**, because it lives inside the
worktree that gets removed along with it. Nothing to clean up, nothing sitting
in your changed files asking to be discarded. Every tab of one agent shares one
worktree, so it does not matter which tab you drop on.

It is inside the worktree rather than tucked away somewhere neutral for a
practical reason: some agent CLIs will not read a file outside the workspace
they were started in, so a folder next door would be a path the agent cannot
open.

> **This changed.** Files dropped on an agent used to land at the root of the
> worktree, visible to git, and every screenshot left an untracked file to
> discard by hand. They now go to `.dux/uploads` instead. Dropping on a terminal
> is unchanged. If you liked the old behavior, see the two settings below.

**On a terminal**, in the folder that terminal is *actually* in right now. Not
the folder it was opened in. If you opened a terminal at your repo root and then
typed `cd docs/images`, a file dropped on it lands in `docs/images`, because
that is where you are. dux asks the live process each time rather than
remembering where it started, and it prefers whatever program is in the
foreground over the shell behind it, on the reasoning that a file handed to a
terminal belongs to whatever is reading it. If that foreground program has
already exited but its job is still running, dux asks a surviving part of that
job before it falls back to the shell, because a pipeline whose first stage
finished is an ordinary thing and the shell is not where you are.

That last step, asking the rest of a job whose first program has already gone, is
**Linux only**. On macOS dux cannot enumerate the job, and it will not pretend a
job it cannot see has finished, so in that one situation it refuses the drop and
asks you to try again rather than quietly saving into the shell's folder instead.
The window is narrow and it closes on its own: drop the file again and the
program now in the foreground answers for itself. Everything else on this page
behaves identically on both platforms.

If dux cannot read the process at all, it refuses the drop and tells you, rather
than writing somewhere else and naming that instead. Being unable to see where a
terminal is has never been a good reason to guess.

**On the editor's file tree**, in the folder you dropped on. This is the
durable answer, and it is the one place where you pick the destination by
pointing at it rather than by having navigated there. Drop on a **folder** row
and the files go into that folder. Drop on a **file** row and they go into the
folder that file is in, because a file is not a place to put a file. Drop on the
**empty space** below the tree and they go to the worktree root. Whichever row
would receive the drop lights up while you are still holding the files, so you
can see where they are going before you let go, and the toast afterwards names
the folder.

Nothing here goes near the upload folder and nothing writes an ignore file.
These are **ordinary, visible files**: `git status` shows them, you can commit
them, and they are still there after the agent is gone. That is the whole point
of dropping them here instead of on the pane. The agent's Changes pane refreshes
immediately, so the new files appear there without waiting for the next poll,
and the file tree and the file search index both pick them up at once.

A tree drop pastes nothing into any terminal. It saves the file and stops, which
is what "add this to my project" means.

You cannot drop outside the worktree or into `.git`, exactly as you cannot
create, rename or move anything into either. dux refuses and says so, and
nothing is written.

A drop is **stricter than create, rename and move in one way**: those three
follow a symlinked directory as long as it stays inside the worktree, so
creating a file inside a `libs -> packages/libs` link works, while a drop
refuses any folder reached through a link. That is the safe direction to be
wrong in, since a link is the one way an ordinary-looking relative path still
lands outside the tree. Drop into the real folder instead.

dux takes **files**, not folders. Drop a folder and it is refused by name, with
any files you dropped alongside it still saved; one toast tells you which was
which. And if the folder you dropped on has been deleted since the tree last
listed it, the drop is refused with that folder named, rather than being
recreated behind your back.

The toast that appears afterwards names the folder for each file, so you never
have to guess. Drop several onto a terminal, type `cd` in the middle, and they
genuinely do land in different folders; the toast says which went where.

If the file landed inside an agent's worktree, that agent's Changes pane is
refreshed straight away rather than on its next poll. With the ignore file in
place an upload changes nothing git can see, so the pane stays exactly as it
was, which is the point; turn the ignore off and your screenshot is sitting
there as an untracked file before you have finished typing the sentence about
it. A file that landed somewhere else, a terminal you had `cd`'d out of the
worktree, or a project or standalone terminal with no agent behind it, changes
nothing git is watching, so nothing claims otherwise.

## Keeping the uploads out of git

The upload folder hides itself. dux keeps a `.gitignore` in it containing a
single `*`, which ignores everything in the folder **including the ignore file
itself**. Run `git status` after dropping a
screenshot and it prints nothing at all: not the image, not the ignore file, not
even the folder.

dux tries to write that file on **every** upload, not only on the drop that
first creates the folder. It costs one exclusive-create syscall that does
nothing when the file is already there, and it means the folder repairs itself:
delete the ignore file, or create the folder while the setting is off and turn
it on later, and the next dropped file puts it back.

Two things dux deliberately will not do:

- **It never edits a `.gitignore` that is already there.** If you have written
  your own rules in that folder, they win, and turning the setting on later will
  not overwrite them.
- **It never touches `.git/info/exclude`.** That looks like the tidier place for
  a local ignore, and in a worktree it is a trap: inside a linked worktree that
  path resolves to the **main checkout's** copy, so writing it would edit your
  main repository and change what git ignores in every other worktree at once.
  Polluting your repo is the thing this feature exists to stop doing.

Both halves are settings, on `[ui]` in `config.toml`:

| Key | Default | What it does |
|---|---|---|
| `upload_directory` | `".dux/uploads"` | Where an agent's dropped and pasted files go, relative to that agent's worktree. Must be a relative path with no `..` in it that a filesystem could actually hold; anything else falls back to the default and says so once in `dux.log`. |
| `upload_write_gitignore` | `true` | Whether to keep the self-ignoring `.gitignore` in that folder, attempted on every upload. Set it to `false` if you intend to commit what you drop or paste, and your uploads show up as ordinary untracked files again. This one is also a row in the web UI's **Preferences** dialog, as *Hide dropped and pasted files from git*. |

`upload_directory` deliberately has **no** Preferences row. It is a path, and a
free-text box is a poor way to pick one; doing it properly needs a directory
picker the dialog does not have. Edit it in `config.toml`.

Set `upload_directory = ""`, or point it at anything outside the worktree, and
dux will not follow you: uploads have to be somewhere the agent can read and
somewhere that dies with the agent, so an unusable value degrades to the default
rather than being obeyed. The same goes for a value the filesystem itself could
not store, one holding a control character or a null byte (a TOML `"\n"` escape
will get you one), or one longer than a path is allowed to be. Every one of them
is caught when the config loads, warned about once in `dux.log`, and replaced.

**A rejected value does not survive in your config file.** The replacement
happens in memory as the config loads, so the next time dux saves `config.toml`
for any reason the corrected value is what gets written and the one you typed is
gone. That is the same treatment `terminal_font_size` gets for an out-of-range
number. If you want to keep the value you wrote while you work out why it was
refused, keep a copy of it somewhere other than `config.toml`.

## Dropped files do not follow a fork

Forking an agent copies its uncommitted work into the new worktree, and it does
that by asking git what has changed. Uploads are invisible to git by design, so
git does not mention them and the fork does not get them: the new agent starts
with an empty upload folder. Turn `upload_write_gitignore` off and they are
ordinary untracked files again, which a fork does carry across. If you want a
particular screenshot in both, drop it onto both.

Want the old behavior back? The nearest thing is
`upload_write_gitignore = false`, which keeps the folder but hands the files
back to git, so they appear in Changes and can be committed. There is
deliberately no way to point uploads at the worktree root itself: the folder is
what makes them easy to ignore, easy to find, and easy to throw away.

## Nothing is ever overwritten

Drop `screenshot.png` twice and you get two files. The second one is saved under
a new name, with a timestamp and a counter, and **the toast tells you the new
name**. That last part matters: a message saying "1 file was renamed" would tell
you something changed without telling you what to type, which is useless when
the whole point is to reference the file.

The file is created exclusively, so two uploads racing each other cannot land on
the same name, and dux refuses to write through a symlink. If something
unexpected is sitting at that name, the drop fails and says so rather than
quietly writing next to it.

The same holds for a drop on the editor's file tree, and it is worth being
precise about how that differs from **moving** a file in the editor. A move is
*refused* when something is already at the destination, because you named that
exact destination and quietly putting the file somewhere else would be a
different operation from the one you asked for. A drop names only a folder, so
the next free name is the honest answer and the toast tells you which one it
used. Neither one ever overwrites what is already there.

## Your filenames are kept as you had them

dux **validates** a dropped name; it does not rewrite it. Accented names,
Japanese names, Cyrillic names, names with spaces, parentheses and apostrophes
all arrive exactly as they were. Rewriting names to "safe" characters destroys
information, and it can quietly turn two different files into one.

A handful of names are refused outright, with the reason named:

- an empty name, or `.` and `..`, which name a folder and not a file
- anything containing a `/`, which makes it a path rather than a name
- control characters and null bytes, which no terminal can print back to you
- anything longer than the filesystem will accept

When a name is right at the length limit and a collision forces a suffix, dux
trims the front of the name to make room and keeps the extension, so the file is
still recognizable as an image.

## The folder has to be nameable too

The name is only the last part of the path, and the path is what ends up in your
prompt, so the folders above it are held to the same standard. dux refuses a drop
into a folder whose own path could not survive the trip to your terminal: one
holding a line feed (which arrives as a submit rather than as text), one holding
an escape character (which the program reading your terminal simply obeys), or
one whose name is not valid text at all. Those are the only refusals, and none
of them is something a different way of writing the path could rescue.

Everything else works. Spaces, dollars, backticks, quotes and semicolons in a
folder name are all fine, which matters because a worktree path is built from
your project's name.

## What the path looks like when it lands

Whether a dropped file is picked up automatically depends on the exact shape of
the path in the prompt, and the agent CLIs do not agree with each other about
what that shape should be. So it is **a per-provider setting**, `web_dragdrop_paste`,
in the provider's own block in `config.toml` next to `command` and `resume_args`.

You almost certainly do not need to touch it. dux ships the value it measured for
each CLI it knows about.

```toml
[providers.codex]
command = "codex"
web_dragdrop_paste = "single_quoted"
```

### The four values

A file at `/home/you/My Project/it's here.png` goes out as:

| Value | What is written into the prompt |
|---|---|
| `bare` | `/home/you/My Project/it's here.png` |
| `single_quoted` | `'/home/you/My Project/it'\''s here.png'` |
| `double_quoted` | `"/home/you/My Project/it's here.png"` |
| `backslash_escaped` | `/home/you/My\ Project/it\'s\ here.png` |

`bare` sends the path exactly as it is on disk. `single_quoted` and
`double_quoted` wrap it so that a shell lexer reads the whole thing as one word,
escaping whatever would otherwise end the quoting. `backslash_escaped` skips the
quotes and protects the significant characters one at a time, which is what
several real terminal emulators do when you drop a file on them.

**Which one do you want?** If a dropped file arrives as plain text instead of
attaching, the CLI probably wants the path quoted, so try `single_quoted`. If the
path arrives visibly mangled, with stray quote or backslash characters in it, the
CLI probably wants it `bare`.

### Which CLI needs which, and why

Every row here was produced by running that CLI's own path handling over the exact
bytes dux sends, not by reading the code and summarising it.

| CLI | What it does with a pasted path | Value |
|---|---|---|
| Claude Code | Trims the whole string, strips one surrounding pair of matching quotes, undoes backslash escapes, then asks whether what is left looks like an image. Never splits on whitespace. | `bare` |
| OpenCode | Strips quote characters off both ends, resolves a `file://` URL, undoes backslash escapes. No shell splitting, so a space is harmless. | `bare` |
| Codex | Strips one matching quote pair, resolves a `file://` URL, and otherwise lexes the text with POSIX shell rules, accepting it only if it comes out as exactly one token. | `single_quoted` |
| Copilot CLI | Closed source, so this one is **not verified**. It is defaulted to `bare`, the do-nothing option and what two of the three CLIs above want. | `bare` (a guess) |

Anything else, including a provider you add yourself, gets `bare` unless you say
otherwise. An absent key means `bare`, and so does a value dux does not recognise
(it says so once in `dux.log` and carries on rather than refusing to load your
config).

The `web_` prefix is there to be obvious about scope: this affects the browser and
nothing else. In the terminal UI, dropping a file onto the window is your terminal
emulator's job and dux is not involved at all.

### What is known to fail

This is the more useful half of the table, and none of it is something dux can fix
from its side. dux sends the correct bytes; the receiving tool rewrites them.

- **Single-quoting a path that contains an apostrophe breaks Claude Code.** POSIX
  quoting writes an embedded apostrophe by closing the quote, escaping it and
  reopening, and Claude Code's own unescaping step then collapses that into three
  apostrophes in a row. A file in a folder called `Bob's app` comes out naming
  nothing. This is why Claude's value is `bare` and why an earlier version of dux
  that quoted everything was wrong.
- **Any form carrying a backslash is mangled by Claude Code's unescaping step.**
  That covers `backslash_escaped` outright, and it also covers a path that simply
  has a backslash in its name, whatever form you send it in. Backslashes in file
  and folder names are rare on macOS and Linux, but when they happen there is no
  form that survives.
- **OpenCode eats a trailing quote character, and unescapes backslashes.** It
  strips quote characters from *both ends* rather than one matching pair, so a
  file whose own name ends in a quote loses that character. And like Claude Code
  it undoes backslash escapes, so a path holding a backslash is mangled there too.
- **Codex ignores a paste that is too long before it ever looks for a path.**
  Anything over 1000 characters is filed away as generic pasted content, and the
  quoting dux adds counts toward that. dux measures the finished paste rather than
  the file's own path, and when it would go over the limit it does not send it at
  all: the toast tells you the file was saved, gives you its full path, and says
  the agent will not pick it up automatically. The limit belongs to Codex itself,
  not to a quoting style and not to what you called the provider block: it applies
  whichever `web_dragdrop_paste` value you give Codex, it follows Codex under any
  block name you like (`[providers.myagent] command = "codex"` still gets it), and
  a block you happened to name `codex` that runs something else does not. dux
  decides by the `command` you configured, comparing on its file name, so a full
  path such as `/usr/local/bin/codex` counts the same as the bare name. No other
  CLI has been measured to have a limit, and a terminal has none at all.

A `file://` URL is deliberately **not** one of the four values. Codex and OpenCode
both resolve one, but whether Claude Code does on its paste path has not been
measured. It is not a rejected idea, it is a candidate: measure it against a CLI,
and it can be added as a fifth value for that provider.

### Getting it wrong is not usually a breakage

If the value is wrong for your CLI, the normal symptom is that the file is not
attached automatically and its path is left sitting in the prompt as ordinary
text. You can still work with that, and you can still refer to the file by the
path you are looking at. Nothing is lost and nothing is overwritten.

## Dropping onto a terminal

A terminal is not an agent, and it does not read `web_dragdrop_paste` at all. Its
dropped paths are **always quoted**, because a terminal runs a shell, and a shell
is precisely the thing that would split a path on its spaces, expand a `$` in it
and run a command substitution the moment you press Enter on the line the path
landed in. dux permits all of those characters in a destination path, so the
quoting is what makes them inert. The path is pasted at your cursor as one
literal word and nothing is submitted for you. There is no length limit either:
that limit is a property of Codex's composer, and a shell does not have one, so a
very long path is sent to a terminal rather than held back.

## Several files at once

Drop a handful and they are saved one after another, and their paths are pasted
**in the order you dropped them**, not in whatever order the uploads happened to
finish. Each path is pasted on its own, followed by a single space and no
newline, because a newline would submit your half-written prompt, and because
these tools only treat a pasted path as an attachment when the whole paste is
that one path.

While the uploads are running you get a spinner naming the file being sent and
counting through the drop, so a large file or a busy server never looks like
nothing happened. It is replaced, in place, by **one** toast reporting the whole
drop rather than one per file.

## Who can drop, and who can paste

This section is about the two **pane** drops. A drop on the editor's file tree
pastes nothing, so nothing below applies to it: it needs no input ownership and
a watcher can do it.

Only the device that currently holds input. Terminals in dux are one-writer,
many-watchers (see [The workspace in the browser](/docs/web-workspace)), and the
drop target only appears for the writer, because a watcher could not paste the
path afterwards anyway.

A watcher who pastes an image is told rather than ignored: nothing is uploaded,
and a toast says the image was not saved and that taking over is the way to
paste it here. Nothing is left on the server to clean up.

If you lose input to another device in the moment between the file being saved
and its path being pasted, dux tells you plainly: the file **was** saved, here
is its full path, and the path was not sent. You can then take over input and
paste it yourself. The same honesty applies on a phone: if the compose box goes
away mid-upload (you rotated to a wide layout, or switched the box off), dux
reports the file as saved-but-not-added with its full path rather than claiming
it joined a message you can no longer see.

## Limits, and switching it off

Two `[server]` settings, both read at startup, so changing either needs a server
restart. See [Server mode overview](/docs/server-mode) for the rest of them.

| Key | Default | What it does |
|---|---|---|
| `file_drop_max_bytes` | `104857600` (100 MiB) | Largest single dropped file. A file over it is refused with a message saying so, and nothing is written. Set to `0` to switch file drop off entirely: the pane stops offering a drop target, and the server refuses any upload that reaches it anyway. |
| `file_drop_max_concurrency` | `2` | How many uploads are accepted at once. This bounds how much upload dux holds in memory, not just how much work it does at a time. An upload beyond the limit waits up to 30 seconds for a slot; if none comes free it is refused with a `503` saying the server is busy, and the browser tells you to try the drop again in a moment. `0` clamps to `1`. |

The size default is generous on purpose. Screenshots from a high-resolution
display are routinely several megabytes, and a stingier limit would reject the
most ordinary thing anyone drops.

## Why this is web-only

The terminal UI needs nothing here, and deliberately gets nothing. Dropping a
file onto a terminal window is your terminal emulator's job, and it already does
it: Alacritty, kitty, Ghostty and the rest all type the path in for you, and the
file is already on the machine the agent is running on. There is nothing for dux
to add. This feature exists to close the gap that only a browser has.

There is no drag gesture on a phone, so dropping is a desktop gesture. Pasting
is not: on a phone the compose bar is your typing surface (see
[The workspace in the browser](/docs/web-workspace)), and pasting an image into
it puts the saved file's path into your draft.
