---
title: Dropping files onto an agent
description: Drag a screenshot from your desktop onto an agent or terminal in the browser and dux saves it on the server, then pastes its path into the prompt. Where files land, why an agent's uploads stay out of git, why nothing is ever overwritten, and why this is web-only.
group: Web UI
order: 67
---

You have a screenshot. Your agent is running on a machine somewhere else, and
you are looking at it through a browser tab. Getting that image in front of the
agent used to mean copying it to the server by hand and typing out the path.

Now you drag it onto the terminal and let go. dux saves the file on the server
and pastes its path into the prompt, ready for you to finish the sentence.

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
terminal is **"put this here"**. It is a file you are placing in a folder you
chose, and you meant it.

Those want opposite things from git, so they get different destinations.

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

The upload folder hides itself. When dux creates it, it writes a `.gitignore`
into it containing a single `*`, which ignores everything in the folder
**including the ignore file itself**. Run `git status` after dropping a
screenshot and it prints nothing at all: not the image, not the ignore file, not
even the folder.

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
| `upload_directory` | `".dux/uploads"` | Where an agent's dropped and pasted files go, relative to that agent's worktree. Must be a relative path with no `..` in it; an absolute or traversing value falls back to the default and says so once in `dux.log`. |
| `upload_write_gitignore` | `true` | Whether to write the self-ignoring `.gitignore` when dux creates that folder. Set it to `false` if you intend to commit what you drop, and your uploads show up as ordinary untracked files again. |

Set `upload_directory = ""`, or point it at anything outside the worktree, and
dux will not follow you: uploads have to be somewhere the agent can read and
somewhere that dies with the agent, so an unusable value degrades to the default
rather than being obeyed.

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

## Who can drop

Only the device that currently holds input. Terminals in dux are one-writer,
many-watchers (see [The workspace in the browser](/docs/web-workspace)), and the
drop target only appears for the writer, because a watcher could not paste the
path afterwards anyway.

If you lose input to another device in the moment between the file being saved
and its path being pasted, dux tells you plainly: the file **was** saved, here
is its full path, and the path was not sent. You can then take over input and
paste it yourself.

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

There is no drag gesture on a phone, so there is no phone surface either. On a
phone, the compose bar is your typing surface (see
[The workspace in the browser](/docs/web-workspace)).
