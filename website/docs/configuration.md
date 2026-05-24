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

Hand-edits are preserved across saves — dux rewrites the file with `toml_edit`, so
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

Because the file holds portable intent — projects, providers, themes, keybindings —
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

Some projects need a little ceremony before an agent is useful — installing
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
`[startup_command_terminal]`, so the behavior stays portable and reviewable.
