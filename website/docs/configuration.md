---
title: Configuration
description: Where the config file lives, how it expands environment variables, and the commands that manage it.
group: Getting started
order: 2
---

dux follows one rule above all others: **the config file is the documentation.**
Every setting is configurable, and every setting is commented inline. You should
never have to leave `config.toml` to understand what an option does.

## Where it lives

dux writes a fully annotated `config.toml` the first time it launches:

- **Linux:** `~/.config/dux/config.toml`
- **macOS:** `~/.dux/config.toml`

Themes are preselected, keybindings are ready to remap, and the default providers
are already wired in. Open it, read the comments, change what you like.

Each provider block carries its own settings, including `web_dragdrop_paste`,
which decides what a dragged and dropped file's path looks like when the web UI
writes it into that agent's prompt. dux ships a measured value for every CLI it
knows about, so it is not something you normally set; see
[Custom agents and providers](/docs/custom-agents) for the anatomy of a provider
block and [Dropping files onto an agent](/docs/dropping-files) for what each value
sends and which CLI wants which.

## Managing the config

A handful of subcommands handle the file without you having to hunt for it:

- `dux config path` prints the absolute path to the active config file.
- `dux config diff` shows what you've changed from the defaults.
- `dux config regenerate` previews the latest canonical template, so you can see
  new options after an upgrade.
- `dux config restore-docs` puts the explanatory comments back into a config that
  lost them, keeping every value exactly as it is.

Hand-edits are preserved across saves: dux rewrites the file with `toml_edit`, so
your comments and ordering survive.

### What `dux config diff` shows, and what it holds back

The summary is derived from the shape of the config itself, not from a list
somebody has to remember to extend, so a setting added in a new release turns up
in your diff the day it ships. It compares the file as written against the
built-in defaults, which means it reports what you typed rather than what dux
normalizes it into: no clamping of out-of-range numbers, and no default provider
block quietly folded into a config that does not name one.

Two things are summarized rather than printed:

- `[env]` reports only `env: changed`. Those values are frequently API tokens,
  and a token that ends up in a terminal scrollback is a token you have to
  rotate.
- `[[projects]]` reports only a count. A project's index is not a stable name, a
  project id can be generated on load, and projects carry their own `env`.

Macros report a count too, since a macro body is arbitrary prose. Everything else
is shown as `setting: default -> yours`, with long values cut off at 40
characters. That makes the summary safe to paste into a bug report.

> **`dux config diff --raw` is not safe to paste.** It prints a unified diff of
> your entire config against the default one, which includes every value in your
> `[env]` table verbatim. Redact it before you share it anywhere.

### Getting the comments back

Older versions of dux could create a `config.toml` with no comments at all, and
because ordinary saves only ever preserve comments (they never add them), such a
file stayed bare forever. `dux config restore-docs` fixes that:

```bash
dux config restore-docs        # preview: shows a diff, changes nothing
dux config restore-docs --yes  # apply
```

It is deliberately careful with your data:

- Every value survives, including projects and their ids, macros with multi-line
  bodies, provider commands and their arguments, and environment values.
- Applying it writes a timestamped backup of the current file first and prints
  where it went.
- Settings dux does not recognize are kept as they are, not quietly deleted, and
  are listed in the output.
- A few sections dux genuinely no longer reads are removed, and the removal is
  reported rather than done silently.
- If the file cannot be parsed, the command refuses and changes nothing rather
  than falling back to defaults, which would throw your settings away.

## Environment variables and portable paths

Project paths understand `$HOME`, `${HOME}`, and `~`, and environment values expand
`${VAR}` from your shell. That means secrets stay as references instead of being
hardcoded:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
env  = { EDITOR = "true", API_KEY = "${FOO_KEY}" }
```

Because the file holds portable intent (projects, providers, themes, keybindings)
rather than runtime state, it's **safe to commit to git.** Drop it in your dotfiles
and it travels between machines without leaking your username or your secrets.

### Keeping secrets out of `[env]`

Anything you type literally into `[env]` (or a project's `env`) is exactly that:
a literal. It sits in `config.toml` on disk, it shows up in
`dux config diff --raw`, and it travels with the file into your dotfiles repo.
Two mechanisms let you avoid that, and both are worth knowing.

**The simplest one is to write nothing at all.** Every agent and every terminal
dux spawns inherits dux's own environment, so a variable that is already
exported in the shell you launched dux from is already there. If `ANTHROPIC_API_KEY`
comes out of your shell profile or a secrets agent, your provider CLI will find
it without `[env]` ever mentioning it. Use `[env]` for the things dux has to add
or override, not for the things you already have.

**When you do need an entry, reference the variable instead of pasting the
value.** `$VAR` and `${VAR}` inside an env value are expanded from *dux's own*
process environment, at the moment a session or terminal is spawned:

```toml
[env]
API_KEY = "${FOO_KEY}"   # resolved from the environment dux itself was launched in
```

Three details that matter:

- The lookup happens in dux's environment, not in the agent's shell, so the
  variable has to be exported *before* dux starts. Launching dux from a shell
  where it is set, or from a wrapper that sources your secrets first, is the
  usual way.
- If the variable is not set, dux does not fail and does not blank the value: it
  leaves the text alone, and the agent receives the literal string `${FOO_KEY}`.
  If a tool starts complaining about a nonsense credential, that is the first
  thing to check.
- `~` is not expanded in an env *value*. Tilde works in a project `path`; for a
  home-relative env value, write `$HOME/...`.

None of this is encryption. It keeps the secret out of the file, which is the
part that gets committed, pasted into issues and synced between machines; the
value still reaches every agent and terminal dux spawns, and dux is a
trusted-access tool with no per-user isolation.

## Keybindings

Every keybinding dux uses is configurable under the `[keys]` section, and the
in-app help overlay is the authoritative reference for what's currently bound.
Bindings are arrays, so an action can answer to more than one key:

```toml
[keys]
quit         = ["q", "ctrl-c"]
open_palette = ["ctrl-p"]
```

Modifier and control-key parsing is case-insensitive: `Ctrl-g`, `ctrl-g`, and
`CTRL-g` all mean the same thing. Letter keys are lowercased too, so to bind an
uppercase letter you write the shifted form, e.g. `shift-p`.

Rather than memorizing hotkeys, you can reach most actions through the terminal
UI's command palette; the in-app help overlay names the key it is currently bound
to. It's the fastest way to discover what dux can do.

## Per-project startup commands

Some projects need a little ceremony before an agent is useful: installing
dependencies, linking an env file, and so on. A project's `startup_command` runs
that ritual for you when an agent's worktree is created:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
startup_command = """
npm install
ln -sfn "$DUX_WORKTREE_PATH/.env.local" .env
"""
```

The shell used to run startup commands is itself configurable under
`[startup_command_terminal]`, so the behavior stays portable and reviewable. For
the full treatment (per-project and global `env`, the `DUX_*` variables dux
injects, and the startup shell), see
[Startup commands & environment variables](/docs/startup-commands).

## Naming a web instance (title + favicon)

When you run several dux servers, it helps to tell their browser tabs apart. Two
web-only settings under `[server]` do that:

```toml
[server]
title   = "dux (prod)"   # the browser tab title and the in-app wordmark
favicon = "blue"         # recolors the duck favicon for this instance
```

`title` drives both the browser `<title>` and the brand wordmark. `favicon` is
empty by default (the original yellow duck); set it to one of the curated tint
colors — `violet`, `blue`, `sky`, `cyan`, `teal`, `green`, `amber`, `orange`,
`red`, `pink`, or `rose` — to recolor the duck so each instance is
distinguishable at a glance.

You don't have to edit the file: from the web UI, open the cog menu and choose
**Preferences…**. The change is written to `[server]` in `config.toml` and applies
to every open tab immediately, so it sticks across restarts.

## Where dropped files go (`[ui]`)

Two web-only settings under `[ui]` decide what happens to a file you drop or
paste onto an **agent** pane. Dropping onto a terminal is unaffected: that
always lands in the folder the terminal is actually in.

```toml
[ui]
upload_directory       = ".dux/uploads"  # relative to the agent's worktree
upload_write_gitignore = true            # hide the uploads from git
```

`upload_directory` is where the file is saved, relative to that agent's
worktree, and the folder is created the first time you drop something. Living
inside the worktree is what lets an agent CLI read it (several refuse to read
outside their workspace) and what makes cleanup free: delete the agent and the
uploads go with it. It has to be a relative path with no `..` in it; an
absolute, traversing or empty value falls back to `.dux/uploads` and says so
once in `dux.log`.

`upload_write_gitignore` keeps a `.gitignore` containing a single `*` in that
folder, which ignores everything in it including itself, so your screenshots
never turn up as untracked files. dux tries to write it on every upload, not
only when it first creates the folder, so the file comes back if you delete it
or if the folder was created while the setting was off. Set it to `false` if you
intend to commit what you drop. dux never edits a `.gitignore` you already have
there, and never writes to `.git/info/exclude` (in a linked worktree that
resolves to the main checkout's copy, so it would change what git ignores in
every other worktree at once).

Neither of these is in the Preferences dialog; they are config-file settings.
The full story is in
[Dropping files onto an agent](/docs/dropping-files).

## Editing settings from the web

The same **Preferences…** dialog is where every web-adjustable setting lives, a
curated set of `[server]`/`[ui]`/`[capabilities]`/`[defaults]` preferences, so you don't
have to hand-edit `config.toml` for the common ones. Rows are grouped by which
surface they affect:

- **This browser (Web)**: the instance name/favicon above, plus
  copy-on-select, desktop notifications, and the Changes pane default.
- **Both surfaces**: status-message auto-clear, the attention indicator and
  its grace period, the always-show-tab-strip preference, the PR banner
  position, clickable hyperlinks, GitHub integration, and whether new agents
  start with a random pet name.

Each row shows its documented default and, where a value of `0` means
something special (like "never auto-clear"), that meaning too. The server
validates and clamps every value before saving, and every connected browser
refreshes automatically once it's written.

Not every `[ui]`/`[capabilities]` field is here: settings that only affect the
terminal UI, like its theme or the diff viewer's tab width and line
numbers, aren't exposed in this web panel. Keybindings,
provider commands, and project identity also stay in the raw config file (or
their own dedicated dialogs) by design; in the web UI, use the cog menu's
**Configuration → Edit config file…** for anything not covered.
