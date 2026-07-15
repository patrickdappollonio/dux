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

## Managing the config

Three subcommands handle the file without you having to hunt for it:

- `dux config path` prints the absolute path to the active config file.
- `dux config diff` shows what you've changed from the defaults.
- `dux config regenerate` previews the latest canonical template, so you can see
  new options after an upgrade.

Hand-edits are preserved across saves: dux rewrites the file with `toml_edit`, so
your comments and ordering survive.

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

## Keybindings

Every keybinding dux uses is configurable under the `[keys]` section, and the
in-app help overlay (`?`) is the authoritative reference for what's currently bound.
Bindings are arrays, so an action can answer to more than one key:

```toml
[keys]
quit         = ["q", "ctrl-c"]
open_palette = ["ctrl-p"]
```

Modifier and control-key parsing is case-insensitive: `Ctrl-g`, `ctrl-g`, and
`CTRL-g` all mean the same thing. Letter keys are lowercased too, so to bind an
uppercase letter you write the shifted form, e.g. `shift-p`.

Rather than memorizing hotkeys, you can reach most actions through the command
palette (`Ctrl-P` by default). It's the fastest way to discover what dux can do.

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

## Editing settings from the web

The same **Preferences…** dialog is where every web-adjustable setting lives, a
curated set of `[ui]`/`[capabilities]`/`[defaults]` preferences, so you don't
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
terminal app, like the TUI theme or the diff viewer's tab width and line
numbers, stay TUI-only and aren't exposed in this web panel. Keybindings,
provider commands, and project identity also stay in the raw config file (or
their own dedicated dialogs) by design; in the web UI, use the cog menu's
**Configuration → Edit config file…** for anything not covered.
