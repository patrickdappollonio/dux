---
title: Dropping and pasting files onto an agent
description: Drag, paste, or pick a file in the browser and dux saves it on the server and pastes its path. Where files land, why agent uploads stay out of git, why an editor drop is an ordinary committable file, and why nothing is ever overwritten.
group: Web UI
order: 67
---

You have a screenshot. Your agent is running on a machine somewhere else, and you are
looking at it through a browser tab. Drag the image onto the terminal and let go: dux saves
it on the server and pastes its path into the prompt, ready for you to finish the sentence.

## Three gestures, one journey

Drag, paste, or pick from a menu. All three save the file on the server and paste its path,
with one toast at the end.

![A dashed outline over the terminal saying the dropped file will be saved and its path pasted.](/screens/file-drop-overlay.png)

**Paste** is the shortest: take the screenshot, press `Ctrl+v` (`Cmd+v` on a Mac), and the
image on your clipboard takes the same route.

**Attach a file…** opens your browser's ordinary file picker, and lives in two places:

- The **Input** group at the top of the pane's own `⋯` menu (see
  [The workspace in the browser](/docs/web-workspace)): the flap's on a phone, the pane
  header's on a computer (the same menu the agent's or terminal's sidebar row opens), the
  floating pill's in theater mode. This is the phone entry point, since there is no drag
  gesture on a touch screen, and on a computer it gives a keyboard-only desktop one. The
  entry appears while that session's terminal is open in front of you and you are the one
  driving it; close the terminal and the entry goes.
- The file tree's right-click menu in the editor, as **Upload here…**, which puts the file
  in the folder you right-clicked and pastes nothing anywhere.

You can pick several files at once, and a picker cannot hand over a folder, so the folder
refusal below never comes up for that gesture.

A few things specific to pasting:

- **Images, and text that is very long, are taken over.** A clipboard carrying an image is
  taken over, and so is text past a length you set
  ([below](#a-very-long-paste-becomes-a-file-too)). Anything shorter pastes exactly as it
  always has. When the clipboard carries an image *and* text, the image wins and the text is
  not pasted.
- **`Ctrl+Shift+v` forces the text** (`Cmd+Shift+v` on a Mac), for that one keystroke. It
  skips image handling entirely. Image-wins is right for a screenshot and wrong for rich
  content: copy a range of spreadsheet cells and the clipboard carries a picture of the cells
  alongside the numbers. It is also how you paste very long text as text.
- **On a phone, the path joins your draft** rather than sending anything. The toast says so.
- **A screenshot usually has no name of its own.** Browsers hand one over as `image.png`, so
  several pastes collide, and dux saves the next one under a new name and tells you what it
  is. With no name at all, dux invents one from the clock, like
  `pasted-2026-08-09-141530.png`.

> [!NOTE]
> dux reads your clipboard only from a real paste, never on demand.

## A very long paste becomes a file too

Paste a 40 KB error log into an agent and every one of those characters goes into its
context window. So past a length you set, dux saves the text as a `.txt` file in the agent's
upload folder and pastes **that file's path** instead. The agent can then open it, scan it,
or grep it.

The threshold is [`ui.upload_pasted_text_chars`](/docs/configuration), and the default is
**4000 characters**, about a long page of prose. Your instructions keep arriving as text
however wordy they get, and a log or a diff becomes a file. Lower it if the command you run
handles long pastes badly.

The details:

- **Counted in characters**, so a paste in Japanese or one full of emoji is measured exactly
  the way an English one is.
- **The file is the pasted text, exactly.** UTF-8, no BOM added, nothing appended, no
  newlines rewritten: carriage returns, trailing spaces, escape codes, combining marks and
  NUL bytes all survive.
- **One exception:** an unpaired surrogate, which UTF-8 cannot represent, is written as the
  replacement character (`U+FFFD`).
- **It is named from the clock**, like `pasted-2026-08-09-141530.txt`, and lands in the same
  folder under the same ignore file and collision-safe naming as everything else here.
- **The toast says what happened and the way out.** It leads with the size that triggered
  it, names the file, and says your text is still on the clipboard and which chord pastes it
  literally: *"That paste was 41320 characters, so dux saved it as a file rather than typing
  it into the agent."* If the save itself fails it says it **tried** to save, and that
  message waits for you instead of clearing itself, because your text is neither typed nor
  saved.
- **`Ctrl+Shift+v` pastes it as text anyway**, for that one keystroke.
- **Set it to `0` to switch it off** and long pastes go to the prompt verbatim.
- **Never in a terminal.** A long paste into a shell is a command, or a heredoc, or a script
  you meant to run, so a terminal pastes text verbatim at any length.
- **In the phone message box too**, where the saved file's **path joins your draft** and you
  write around it before pressing Send.

## Why a path and not the file itself

No agent CLI reads a file out of its input stream. They take a **path**, or they read the
clipboard of the machine they are running on, which when you are driving from a browser is
the wrong computer. That is what every terminal emulator does too: a drop types the path in.
dux does the same, with the one extra step a browser needs: it puts the file on the server
first.

## Where the file lands

Handing an image to an agent is "look at this for me": scratch. Dropping a file into a
terminal or onto the editor's file tree is "put this here": a file you are placing in a
folder you chose. So dux gives them different destinations.

### On an agent's pane

The file goes in that agent's upload folder: `.dux/uploads` inside the agent's worktree,
created the first time you drop something.

- It is **invisible to git** and **deleted with the agent**, so there is nothing to clean up
  and nothing sitting in your changed files.
- Every tab of one agent shares one worktree, so it does not matter which tab you drop on.
- It lives inside the worktree because some agent CLIs will not read a file outside the
  workspace they were started in.

### On a terminal

The file goes in the folder that terminal is *actually* in right now, not the one it opened
in. Open a terminal at your repo root, type `cd docs/images`, and a dropped file lands in
`docs/images`. dux prefers whatever program is in the foreground over the shell behind it.

> [!IMPORTANT]
> If a foreground program has exited but its job is still running, dux looks at the rest of
> the job before falling back to the shell:
>
> - **Linux:** it does, and the drop goes where the job is.
> - **macOS:** dux refuses the drop in that one situation and asks you to try again, rather
>   than quietly saving into the shell's folder. Drop the file again and the program now in
>   the foreground answers for itself.
>
> Everything else on this page behaves the same on both platforms.

If dux cannot read the process at all, it refuses the drop and tells you rather than writing
somewhere else and naming that instead. A file that lands inside an agent's worktree shows
up in that agent's Changes pane straight away; one that lands anywhere else changes nothing
git is watching, and nothing claims otherwise.

Drop several onto a terminal, type `cd` in the middle, and they genuinely do land in
different folders; the toast names the folder for each file.

### On the editor's file tree

The file goes in the folder you dropped on:

- Drop on a **folder** row and the files go into that folder.
- Drop on a **file** row and they go into the folder that file is in.
- Drop on the **empty space** below the tree and they go to the editor's root (the worktree,
  for an agent).

The receiving row lights up while you are still holding the files, and the toast afterwards
names the folder. A tree drop pastes nothing into any terminal.

Nothing here goes near the upload folder and nothing writes an ignore file. These are
**ordinary, visible files**: `git status` shows them, you can commit them, and they are still
there after the agent is gone. The agent's Changes pane, the file tree and the file search
all pick them up at once.

You cannot drop outside the worktree or into `.git`, exactly as you cannot create, rename or
move anything into either. dux refuses and says so, and nothing is written.

> [!IMPORTANT]
> A drop is **stricter than create, rename and move in one way.** Those three follow a
> symlinked directory as long as it stays inside the worktree, so creating a file inside a
> `libs -> packages/libs` link works. A drop refuses any folder reached through a link.
>
> Drop into the real folder instead.

### Odd drops

- **Folders.** dux takes **files**, not folders. Drop a folder and it is refused by name,
  with any files you dropped alongside it still saved, and one toast says which was which.
- **A folder that was deleted** since the tree last listed it: the drop is refused with that
  folder named, rather than the folder being recreated.
- **Unreadable destination paths.** The whole path ends up in your prompt, so dux refuses a
  drop into a folder whose path holds any control character (a line feed arrives as a
  submit, an escape character is obeyed by the program reading your terminal), text that is
  not valid UTF-8, or a path that no longer names that folder. Spaces, dollars, backticks,
  quotes and semicolons in a folder name are all fine, which matters because a worktree path
  is built from your project's name.

## Keeping the uploads out of git

The upload folder hides itself. dux keeps a `.gitignore` in it containing a single `*`, which
ignores everything in the folder **including the ignore file itself**. Run `git status` after
dropping a screenshot and it prints nothing at all: not the image, not the ignore file, not
even the folder. With the ignore file in place the Changes pane stays as it was; turn the
ignore off and your screenshot shows up there as an untracked file straight away.

dux tries to create that file on **every** upload, not only the first, so the folder repairs
itself: delete the ignore file, or create the folder while the setting is off and turn it on
later, and the next dropped file puts it back.

Two things dux will not do:

- **It never edits a `.gitignore` that is already there.** Your own rules in that folder win,
  and turning the setting on later will not overwrite them.
- **It never touches `.git/info/exclude`.** Inside a linked worktree that file belongs to the
  main checkout, so writing it would change what git ignores in every other worktree.

### The settings

These are settings, on `[ui]` in `config.toml`:

| Key | Default | What it does |
|---|---|---|
| `upload_directory` | `".dux/uploads"` | Where an agent's dropped and pasted files go. |
| `upload_write_gitignore` | `true` | Keep the self-ignoring `.gitignore` in that folder. |
| `upload_pasted_text_chars` | `4000` | How long a text paste into an agent may be before it becomes a file. |

**`upload_directory`**

- Relative to that agent's worktree. It must be a relative path with no `..` in it that a
  filesystem could actually hold.
- Anything else falls back to the default and says so once in `dux.log`: an empty value, a
  path outside the worktree, one holding a control character or a null byte (a TOML `"\n"`
  escape will get you one), or one longer than a path is allowed to be. Uploads have to be
  somewhere the agent can read and somewhere that dies with the agent.
- It deliberately has **no** Preferences row: picking a path properly needs a directory
  picker the dialog does not have. Edit it in `config.toml`.
- There is deliberately no way to point uploads at the worktree root itself: the folder is
  what makes them easy to ignore, easy to find, and easy to throw away.

**`upload_write_gitignore`**

- Set it to `false` if you intend to commit what you drop or paste; your uploads then show up
  as ordinary untracked files.
- Also a row in the web UI's **Preferences** dialog, as *Hide dropped and pasted files from
  git*.

**`upload_pasted_text_chars`**

- Counted in characters. `0` switches it off.
- Values between 1 and 199, or above 100000, are clamped with one warning in `dux.log`.
- Also a row in the web UI's **Preferences** dialog, as *Save long pastes as a file*.
- Never applies to a terminal.

> [!WARNING]
> **A rejected value does not survive in your config file.** The correction happens as the
> config loads, so the next time dux saves `config.toml` for any reason the corrected value
> is what gets written and the one you typed is gone.
>
> Keep a copy elsewhere while you work out why it was refused. (`terminal_font_size` gets the
> same treatment for an out-of-range number.)

## Dropped files do not follow a fork

Forking an agent copies its uncommitted work into the new worktree by asking git what has
changed. Uploads are invisible to git, so the fork does not get them and the new agent starts
with an empty upload folder. Turn `upload_write_gitignore` off and they become ordinary
untracked files, which a fork does carry across. To have a particular screenshot in both,
drop it onto both.

## Nothing is ever overwritten

Drop `screenshot.png` twice and you get two files. The second is saved under a new name, with
a timestamp and a counter, and **the toast tells you the new name**, because the whole point
is to reference the file.

dux refuses to write through a symlink, and if something unexpected is sitting at that name
the drop fails and says so rather than quietly writing next to it.

A drop on the editor's file tree behaves the same way, and differs from **moving** a file in
the editor: a move is *refused* when something is already at the destination, because you
named that exact destination, while a drop names only a folder, so the next free name is the
honest answer. Neither overwrites.

## How a dropped file is named

**On a terminal or the editor's file tree, your filename is kept exactly as you had it.**
Accented, Japanese and Cyrillic names, names with spaces, parentheses and apostrophes all
arrive exactly as they were. Those folders are yours, and so is what the file is called.

**In an agent's upload folder, the name is cleaned up first.** Agent CLIs misread a pasted
path whose file name holds quotes, apostrophes or backslashes, so dux saves the file under a
plain version of its name: lowercase letters, digits and dashes, with the extension kept.

| You drop | Saved in the upload folder as |
|---|---|
| `Report Q3 (draft).PDF` | `report-q3-draft.pdf` |
| `it's "final".txt` | `it-s-final.txt` |
| `Снимок экрана.png` | `snimok-ekrana.png` |
| `archive.tar.gz` | `archive-tar.gz` |
| `.env` | `.env` |

- Letters from other alphabets are spelled out in Latin ones.
- Only the last extension is kept as an extension.
- A hidden file keeps its leading dot.
- A name with nothing usable left in it, such as one made only of punctuation, is saved as
  `file` with its extension.

The toast names both, so you always know what the file is now called. The upload folder is
scratch that nobody browses by name, which is why this happens there and nowhere else.

### Names that are refused

A handful of names are refused outright, wherever you drop them, with the reason named:

- an empty name, or `.` and `..`, which name a folder and not a file;
- anything containing a `/`, which makes it a path rather than a name;
- control characters and null bytes, which no terminal can print back to you;
- anything longer than the filesystem will accept.

When a name is right at the length limit and a collision forces a suffix, or cleaning up a
name for the upload folder makes it longer than the limit (spelling out another alphabet can),
dux trims the name to make room and keeps the extension.

## What the path looks like when it lands

Whether a dropped file is picked up automatically depends on the exact shape of the path in
the prompt, and the agent CLIs disagree about what that shape should be. So it is **a
per-provider setting**, `web_dragdrop_paste`, in the provider's own block in `config.toml`
next to `command` and `resume_args`.

> [!TIP]
> You almost certainly do not need to touch it. dux ships the value it measured for each CLI
> it knows about.

```toml
[providers.codex]
command = "codex"
web_dragdrop_paste = "single_quoted"
```

### The four values

A file at `/home/you/Bob's Project/.dux/uploads/shot.png` goes out as:

| Value | What is written into the prompt |
|---|---|
| `bare` | `/home/you/Bob's Project/.dux/uploads/shot.png` |
| `single_quoted` | `'/home/you/Bob'\''s Project/.dux/uploads/shot.png'` |
| `double_quoted` | `"/home/you/Bob's Project/.dux/uploads/shot.png"` |
| `backslash_escaped` | `/home/you/Bob\'s\ Project/.dux/uploads/shot.png` |

- `bare` sends the path exactly as it is on disk.
- `single_quoted` and `double_quoted` wrap it so a shell lexer reads the whole thing as one
  word.
- `backslash_escaped` protects the significant characters one at a time, which is what
  several real terminal emulators do.

**Which one do you want?**

- If a dropped file arrives as plain text instead of attaching, the CLI probably wants the
  path quoted, so try `single_quoted`.
- If the path arrives visibly mangled, with stray quote or backslash characters, the CLI
  probably wants it `bare`.

An absent key means `bare`, and so does a value dux does not recognise (it says so once in
`dux.log` and carries on rather than refusing to load your config).

The `web_` prefix marks the scope: this affects the browser and nothing else. In the terminal
UI, dropping a file onto the window is your terminal emulator's job.

### Which CLI needs which, and why

| CLI | Value | Why |
|---|---|---|
| Claude Code | `bare` | Strips one matching quote pair and undoes backslash escapes; never splits on whitespace. |
| OpenCode | `bare` | Strips quote characters, resolves `file://` and undoes backslash escapes; a space is harmless. |
| Codex | `single_quoted` | Lexes the text with POSIX shell rules and accepts exactly one token. |
| Copilot CLI | `bare` (a guess) | Closed source, so **not verified**; `bare` is the do-nothing option. |

Anything else, including a provider you add yourself, gets `bare`.

### What is known to fail

None of this is something dux can fix from its side. dux sends the correct bytes; the
receiving tool rewrites them. The file's own name is not the trouble, because the upload
folder [cleans it up](#how-a-dropped-file-is-named). What is left is the **folder** part of
the path: your project's folder or home folder, the upload folder you configured, or a
standalone agent's own folder.

- **Single-quoting a path that contains an apostrophe breaks Claude Code.** POSIX quoting
  writes an embedded apostrophe by closing the quote, escaping it and reopening, and Claude
  Code's own unescaping collapses that into three apostrophes in a row. A file in a folder
  called `Bob's app` comes out naming nothing. This is why Claude's value is `bare`.
- **A backslash in a folder name is mangled by Claude Code and OpenCode.** Both undo
  backslash escapes, so `backslash_escaped` fails outright, and a folder that simply has a
  backslash in its name fails whatever form you send it in. Rare on macOS and Linux, but when
  it happens no form survives. Renaming the folder is the fix.
- **Codex ignores a paste that is too long before it ever looks for a path.** Anything over
  1000 characters is filed away as generic pasted content, and the quoting dux adds counts
  toward that.
  - dux measures the finished paste, and when it would go over the limit it does not send it
    at all. The report tells you the file was saved, gives its full path, and says the agent
    will not pick it up automatically. That report waits for you rather than clearing itself.
  - The limit applies whichever `web_dragdrop_paste` value you give it.
  - dux decides it is Codex by the `command` you configured, compared on its file name, so
    `[providers.myagent] command = "codex"` and `/usr/local/bin/codex` both count, while a
    block you happened to name `codex` that runs something else does not.
  - No other CLI has been measured to have a limit, and a terminal has none at all.

A `file://` URL is deliberately **not** one of the four values. Codex and OpenCode both
resolve one, but whether Claude Code does on its paste path has not been measured.

If the value is wrong for your CLI, the usual symptom is that the file is not attached
automatically and its path sits in the prompt as ordinary text. You can still refer to the
file by that path. Nothing is lost and nothing is overwritten.

## Dropping onto a terminal

A terminal does not read `web_dragdrop_paste` at all. Its dropped paths are **always
quoted**, because a shell would otherwise split the path on its spaces, expand a `$` and run
a command substitution the moment you press Enter. dux permits all of those characters in a
destination path, so the quoting is what makes them inert.

The path is pasted at your cursor
as one literal word and nothing is submitted for you. There is no length limit either: that
limit belongs to Codex's composer, and a shell does not have one.

## Several files at once

Drop a handful and their paths are pasted **in the order you dropped them**, not the order
the uploads finished. Each path is pasted on its own followed by a single space and no
newline, because a newline would submit your half-written prompt, and because these tools
only treat a pasted path as an attachment when the whole paste is that one path.

While the uploads run you get a spinner naming the file being sent and counting through the
drop. It is replaced, in place, by **one** toast reporting the whole drop rather than one per
file.

## Who can drop, and who can paste

Only the device that currently holds input can drop or paste. Terminals in dux are one-writer,
many-watchers (see [The workspace in the browser](/docs/web-workspace)), and the drop target
only appears for the writer. This applies to the two **pane** drops and to a file attached
from the input or row menus. A drop on the editor's file tree pastes nothing, so it needs no
input ownership, and a watcher can do it, **Upload here…** included.

A watcher who pastes an image is told rather than ignored: nothing is uploaded, and a toast
says the image was not saved and that taking over is the way to paste it here.

> [!IMPORTANT]
> If you lose input to another device between the file being saved and its path being pasted,
> dux tells you plainly: the file **was** saved, here is its full path, and the path was not
> sent.
>
> Take over input and paste it yourself. That message waits for you instead of clearing
> itself, because it holds the only copy of that path on screen.

The same applies on a phone: if the compose box goes away mid-upload (you rotated to a wide
layout, or switched the box off), dux reports the file as saved-but-not-added with its full
path rather than claiming it joined a message you can no longer see.

## Limits, and switching it off

Two `[server]` settings. See [Server mode overview](/docs/server-mode) for the rest.

| Key | Default | What it does |
|---|---|---|
| `file_drop_max_bytes` | `104857600` (100 MiB) | Largest single dropped file. |
| `file_drop_max_concurrency` | `2` | How many uploads are accepted at once. |

**`file_drop_max_bytes`**

- A file over the limit is refused with a message saying so, and nothing is written.
- Set it to `0` to switch file drop off entirely: the pane stops offering a drop target, and
  the server refuses any upload that reaches it anyway.
- The default is generous on purpose: screenshots from a high-resolution display are
  routinely several megabytes.

**`file_drop_max_concurrency`**

- This bounds how much upload dux holds in memory, not just how much work it does at a time.
- An upload beyond the limit waits up to 30 seconds for a slot. If none comes free it is
  refused with a `503` saying the server is busy, and the browser tells you to try the drop
  again in a moment.
- `0` clamps to `1`.

> [!IMPORTANT]
> Both are read at startup, so changing either needs a **server restart**.

## Why this is web-only

Dropping a file onto a terminal window is your terminal emulator's job, with the file already
on the machine the agent runs on. This feature closes the gap that only a browser has.

There is no drag gesture on a phone, so dropping is a desktop gesture. Pasting is not: on a
phone the compose bar is your typing surface (see
[The workspace in the browser](/docs/web-workspace)), and pasting an image into it puts the
saved file's path into your draft. Neither is **Attach a file…**, which works everywhere.
