# dux

`dux` is a terminal UI for managing AI coding sessions per git worktree.

## Features

- Left pane for projects and worktree sessions
- Center pane for live ACP agent output or file diffs
- Right pane split into changed files and a manual shell
- Resizable panes with keyboard shortcuts
- Config written to `~/.dux/config.toml` on macOS
- Config written to `$XDG_CONFIG_HOME/dux/config.toml` or `~/.config/dux/config.toml` on Linux
- Session metadata stored alongside the config directory
- Per-session git worktrees with Docker-style branch names
- ACP session restore when the provider adapter supports `session/load`

## Install

If `~/.cargo/bin` is on your `PATH`, install the binary and run it directly:

```bash
cargo install --path .
dux
```

There is also a helper script:

```bash
./scripts/install.sh
```

On first launch, `dux` creates the config file in the platform-specific dux config directory with the full default configuration and comments.

## Controls

- `Tab`: move focus between panes
- `Shift-Tab`: move focus backward
- `Ctrl-p`: open the command palette
- `:`: open direct command mode
- `Ctrl-w`: toggle resize mode
- In resize mode:
  - `h` / `l`: resize left and right side panes
  - `j` / `k`: resize the right split
- `?`: toggle help
- `q`: quit

### Left pane

- `j` / `k`: move through projects and sessions
- `p`: open the built-in repo browser
- `P`: open manual path entry
- `a`: create a worktree-backed agent session from the selected project
- `d`: cycle the selected project's default provider
- `r`: reconnect a detached ACP session
- `u`: refresh the selected project with `git pull --ff-only`
- `x`: delete the selected worktree session

### Center pane

- `i`: start an ACP prompt turn for the running agent
- `Esc`: leave input mode or close diff view

### Files pane

- `j` / `k`: move through changed files
- `Enter`: open the selected diff in the center pane

### Shell pane

- `i`: send input to the shell process

## Provider setup

The provider commands in the dux config file must point to ACP-compatible adapters for Claude and Codex. `dux` launches those adapters over stdio, initializes ACP, creates or reloads sessions, and streams `session/update` events into the center pane.

## Logging

`dux` writes runtime logs under the dux config directory and the log settings live in the `[logging]` section of the config file.

- `level = "error" | "info" | "debug"`
- `path = "dux.log"` uses a path relative to the dux config directory
- absolute paths also work if you want the log elsewhere
