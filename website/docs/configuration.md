---
title: Configuration
description: Where the config file lives, how it expands environment variables, and the commands that manage it.
group: Getting started
order: 2
---

dux follows one rule above all others: **the config file is the documentation.** Every
setting is configurable, and every setting is commented inline. You should never have to
leave `config.toml` to understand what an option does.

## Where it lives

dux writes a fully annotated `config.toml` the first time it launches:

- **Linux:** `~/.config/dux/config.toml`
- **macOS:** `~/.dux/config.toml`

Themes are preselected, keybindings are ready to remap, and the default providers are
already wired in. Open it, read the comments, change what you like.

Each provider block carries its own settings, including `web_dragdrop_paste`, which
decides what a dragged, dropped or pasted file's path looks like when the web UI writes it
into that agent's prompt. dux ships a measured value for every CLI it knows about, so you
do not normally set it. See [Custom agents and providers](/docs/custom-agents) for the
anatomy of a provider block, and
[Dropping and pasting files onto an agent](/docs/dropping-files) for which CLI wants which
value.

## Managing the config

A handful of subcommands handle the file without you hunting for it:

- `dux config path` prints the absolute path to the active config file.
- `dux config diff` shows what you have changed from the defaults.
- `dux config regenerate` previews the latest canonical template, so you can see new
  options after an upgrade.
- `dux config restore-docs` puts the explanatory comments back into a config that lost
  them, keeping every value exactly as it is.

Hand-edits are preserved across saves: your comments and ordering survive.

### What `dux config diff` shows, and what it holds back

The summary compares your file as written against the built-in defaults, so it reports
what you typed rather than what dux normalizes it into: no clamping of out-of-range
numbers, and no default provider block quietly folded in. A setting added in a new release
turns up in your diff the day it ships, with no list for anyone to maintain.

Two things are summarized rather than printed:

- `[env]` reports only `env: changed`. Those values are frequently API tokens, and a
  token in a terminal scrollback is a token you have to rotate.
- `[[projects]]` reports only a count, because projects carry their own `env` and a
  project's index is not a stable name.

Macros report a count too, since a macro body is arbitrary prose. Everything else is
shown as `setting: default -> yours`, with long values cut off at 40 characters.

> [!CAUTION]
> The plain summary is safe to paste into a bug report. **`dux config diff --raw` is
> not.** It prints a unified diff of your entire config, including every value in your
> `[env]` table verbatim. Redact it before you share it anywhere.

### Getting the comments back

Ordinary saves preserve comments rather than adding them, so a `config.toml` that never
had any stays bare forever. `dux config restore-docs` fixes that:

```bash
dux config restore-docs        # preview: shows a diff, changes nothing
dux config restore-docs --yes  # apply
```

It is careful with your data:

- Every value survives: projects and their ids, macros with multi-line bodies, provider
  commands and their arguments, and environment values.
- Applying it writes a timestamped backup first and prints where it went.
- Settings dux does not recognize are kept as they are, not quietly deleted, and are
  listed in the output.
- A few sections dux genuinely no longer reads are removed, and the removal is reported.
- If the file cannot be parsed, the command refuses and changes nothing rather than
  falling back to defaults, which would throw your settings away.

## Environment variables and portable paths

Project paths understand `$HOME`, `${HOME}`, and `~`, and environment values expand
`$VAR` and `${VAR}` from your shell:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
env  = { EDITOR = "true", API_KEY = "${FOO_KEY}" }
```

> [!TIP]
> The file holds portable intent (projects, providers, themes, keybindings) rather than
> runtime state, so it is **safe to commit to git**. Drop it in your dotfiles and it
> travels between machines without leaking your username or your secrets.

### Keeping secrets out of `[env]`

Anything you type literally into `[env]`, or a project's `env`, is exactly that: a
literal. It sits in `config.toml` on disk, it shows up in `dux config diff --raw`, and it
travels with the file into your dotfiles repo. Two ways to avoid that:

**Write nothing at all.** Every agent and terminal dux spawns inherits dux's own
environment, so a variable already exported in the shell you launched dux from is already
there. If `ANTHROPIC_API_KEY` comes out of your shell profile or a secrets agent, your
provider CLI finds it without `[env]` mentioning it. Use `[env]` for what dux has to add
or override.

**When you do need an entry, reference the variable instead of pasting the value.**

```toml
[env]
API_KEY = "${FOO_KEY}"   # resolved from the environment dux itself was launched in
```

Three details that matter:

- The lookup happens in dux's environment, not the agent's shell, so the variable must be
  exported *before* dux starts. Launch dux from a shell where it is set, or from a
  wrapper that sources your secrets first.
- If the variable is not set, dux does not fail and does not blank the value. It leaves
  the text alone, and the agent receives the literal string `${FOO_KEY}`. When a tool
  complains about a nonsense credential, check this first.
- `~` is not expanded in an env *value*. Tilde works in a project `path`; for a
  home-relative env value, write `$HOME/...`.

> [!WARNING]
> None of this is encryption. It keeps the secret out of the file, which is the part that
> gets committed, pasted into issues and synced between machines. The value still reaches
> every agent and terminal dux spawns, and dux is a trusted-access tool with no per-user
> isolation.

## Keybindings

Every keybinding dux uses is configurable under `[keys]`, and the in-app help overlay is
the authoritative reference for what is currently bound. Bindings are arrays, so an
action can answer to more than one key:

```toml
[keys]
quit         = ["q", "ctrl-c"]
open_palette = ["ctrl-p"]
```

Modifier and control-key parsing is case-insensitive: `Ctrl-g`, `ctrl-g`, and `CTRL-g` all
mean the same thing. Letter keys are lowercased, so bind an uppercase letter as its
shifted form, such as `shift-p`.

> [!IMPORTANT]
> While you type into an agent in the windowed pane, any chord bound here belongs to dux
> and never reaches the agent. Rebinding dux's side is how you free a chord the agent
> needs. The `input-debugging` palette command shows what dux receives for each keypress.
> Full story in [Introduction](/docs/introduction#the-three-panes).

Rather than memorizing hotkeys, reach most actions through the terminal UI's command
palette. It is the fastest way to discover what dux can do.

## Per-project startup commands

A project's `startup_command` runs setup for you when an agent's worktree is created:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
startup_command = """
npm install
ln -sfn "$DUX_PROJECT_PATH/.env.local" .env
"""
```

The shell that runs it is itself configurable under `[startup_command_terminal]`. For the
full treatment (per-project and global `env`, the `DUX_*` variables dux injects, and the
startup shell), see
[Startup commands & environment variables](/docs/startup-commands).

## Naming a web instance (title + favicon)

Running several dux servers? Two web-only settings under `[server]` tell their browser
tabs apart:

```toml
[server]
title   = "dux (prod)"   # the browser tab title and the in-app wordmark
favicon = "blue"         # recolors the duck favicon for this instance
```

`title` drives both the browser tab title and the brand wordmark. `favicon` is empty by
default (the original yellow duck); set it to one of the curated tints: `violet`, `blue`,
`sky`, `cyan`, `teal`, `green`, `amber`, `orange`, `red`, `pink`, or `rose`.

Both are also in the web UI's cog menu under **Preferences…**. The change is written to
`[server]` in `config.toml` and applies to every open tab immediately.

## Where dropped and pasted files go (`[ui]`)

Three web-only settings under `[ui]` decide what happens to a file you drop or paste onto
an **agent** pane:

```toml
[ui]
upload_directory         = ".dux/uploads"  # relative to the agent's worktree
upload_write_gitignore   = true            # hide the uploads from git
upload_pasted_text_chars = 1000            # longer pastes become a .txt file
```

`upload_directory` is where the file is saved, relative to that agent's worktree, created
the first time you drop something. It lives inside the worktree so the agent CLI can read
it, since several refuse to read outside their workspace, and so deleting the agent takes
the uploads with it.

> [!IMPORTANT]
> `upload_directory` must be a relative path with no `..` in it. An absolute, traversing
> or empty value falls back to `.dux/uploads` and says so once in `dux.log`.

`upload_write_gitignore` keeps a `.gitignore` containing a single `*` in that folder,
which ignores everything in it including itself, so your screenshots never turn up as
untracked files. dux rewrites it on every upload, so the file comes back if you delete it
or if the folder was created while the setting was off. Set it to `false` if you intend
to commit what you drop or paste. dux never edits a `.gitignore` you already have there,
and never writes to `.git/info/exclude`, which in a linked worktree would change what git
ignores in every other worktree at once.

`upload_pasted_text_chars` is the point at which text you PASTE into an agent stops being
typed at the prompt and becomes a document: dux saves it as a `.txt` file in the folder
above and pastes that file's path, which costs the agent's context window far less than a
wall of text. The default of 1000 is deliberately conservative, and it counts CHARACTERS,
so a paste in Japanese is measured the way an English one is.

Set it to `0` to always paste text as text, or press `Ctrl+Shift+v` (`Cmd+Shift+v` on a
Mac) to bypass it for one paste. Values between 1 and 199, or above 100000, are clamped
with one warning in `dux.log`.

Dropping or pasting onto a **terminal** is unaffected by all three. That always lands in
the folder the terminal is actually in, and a terminal never turns a paste into a file.

`upload_write_gitignore` and `upload_pasted_text_chars` are rows in the web UI's
**Preferences** dialog, as *Hide dropped and pasted files from git* and *Save long pastes
as a file*. `upload_directory` is not: it is a path, and a free-text box is a poor way to
pick one. The full story is in
[Dropping and pasting files onto an agent](/docs/dropping-files).

## Editing settings from the web

The **Preferences…** dialog holds every web-adjustable setting, a curated set of
`[server]`, `[ui]`, `[capabilities]` and `[defaults]` preferences, grouped by which
surface they affect:

- **This browser (Web)**: the instance name and favicon above, plus copy-on-select,
  desktop notifications, the Changes pane default, and whether dropped and pasted files
  stay hidden from git.
- **Both surfaces**: status-message auto-clear, the attention indicator and its grace
  period, the always-show-tab-strip preference, the PR banner position, clickable
  hyperlinks, GitHub integration, and whether new agents start with a random pet name.

Each row shows its documented default and, where `0` means something special like "never
auto-clear", that meaning too. Values are validated and clamped on save, and every
connected browser refreshes once it is written.

Settings that only affect the terminal UI, such as its theme or the diff viewer's tab
width and line numbers, are not in this panel. Keybindings, provider commands, and project
identity stay in the raw config file or their own dialogs. Use the cog menu's
**Configuration → Edit config file…** for anything not covered.
