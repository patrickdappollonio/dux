---
title: Dropping files onto an agent
description: Drag a screenshot from your desktop onto an agent or terminal in the browser and dux saves it on the server, then pastes its path into the prompt. Where files land, why nothing is ever overwritten, and why this is web-only.
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

## Where the file lands

**On an agent's terminal**, at the root of that agent's worktree. That is
deliberate: the file is inside the repo, so git can see it, and the agent can
commit it along with whatever it does with it. Every tab of one agent shares one
worktree, so it does not matter which tab you drop on.

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

If dux cannot read the process at all, it refuses the drop and tells you, rather
than writing somewhere else and naming that instead. Being unable to see where a
terminal is has never been a good reason to guess.

The toast that appears afterwards names the folder for each file, so you never
have to guess. Drop several onto a terminal, type `cd` in the middle, and they
genuinely do land in different folders; the toast says which went where.

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
one whose name is not valid text at all. Quoting does not help with any of those,
because none of them is a shell problem.

Everything that *is* only a quoting problem still works. Spaces, dollars,
backticks, quotes and semicolons in a folder name are all fine, which matters
because a worktree path is built from your project's name.

## Several files at once

Drop a handful and they are saved one after another, and their paths are pasted
**in the order you dropped them**, not in whatever order the uploads happened to
finish. Each path is pasted on its own, followed by a single space and no
newline, because a newline would submit your half-written prompt, and because
these tools only treat a pasted path as an attachment when it is a single token
on its own.

You get **one** toast for the whole drop rather than one per file.

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
| `file_drop_max_concurrency` | `2` | How many uploads are accepted at once. This bounds how much upload dux holds in memory, not just how much work it does at a time. An upload beyond the limit waits its turn rather than being refused. `0` clamps to `1`. |

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
