# dux

`dux` is a terminal UI for managing AI coding sessions per git worktree.

## Features

- Left pane for projects and worktree sessions
- Center pane for live ACP agent output or file diffs
- Right pane for changed files and diffs
- Resizable panes with keyboard shortcuts
- Config written to `~/.dux/config.toml` on macOS
- Config written to `$XDG_CONFIG_HOME/dux/config.toml` or `~/.config/dux/config.toml` on Linux
- Session metadata stored alongside the config directory
- Per-session git worktrees with Docker-style branch names
- ACP session restore when the provider adapter supports `session/load`

## Install

Download the latest binary for your platform from the [GitHub Releases](https://github.com/patrickdappollonio/dux/releases) page. Extract the archive and place the `dux` binary somewhere on your `PATH` (e.g. `/usr/local/bin`).

On first launch, `dux` creates the config file in the platform-specific dux config directory with the full default configuration and comments.

## Provider setup

The provider commands in the dux config file must point to ACP-compatible adapters for Claude and Codex. `dux` launches those adapters over stdio, initializes ACP, creates or reloads sessions, and streams `session/update` events into the center pane.

## Logging

`dux` writes runtime logs under the dux config directory and the log settings live in the `[logging]` section of the config file.

- `level = "error" | "info" | "debug"`
- `path = "dux.log"` uses a path relative to the dux config directory
- absolute paths also work if you want the log elsewhere
