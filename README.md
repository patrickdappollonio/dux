# dux

`dux` is a terminal UI for managing AI coding sessions per git worktree.

## How It Works

`dux` spawns AI CLI tools (`claude`, `codex`, or any terminal command) directly in a pseudo-terminal (PTY) and renders their output in real time. There is no protocol layer, no adapter binaries, and no JSON-RPC — just the official CLI running exactly as it would in your terminal.

This means:

- **Bring any CLI.** Any AI coding tool that runs in a terminal works with dux. Configure the command and args in `config.toml` and you're set.
- **No banning risks.** You're running the official CLI the way it was designed to be used — the same binary, the same auth, the same API calls.
- **Full CLI feature support.** Hooks, MCP servers, skills, slash commands, permission dialogs, thinking indicators — everything the CLI supports works out of the box because dux is just hosting the real terminal session.
- **Crash recovery.** If dux crashes, the agent process dies — and that's fine. Just start a new session on the same worktree. The worktree and all file changes are preserved.

## Features

- Left pane for projects and worktree sessions
- Center pane for live agent terminal output or file diffs
- Right pane for changed files and diffs
- Resizable panes with keyboard shortcuts
- Collapsible project sidebar
- Command palette with fuzzy search
- Config written to `~/.config/dux/config.toml` (Linux) or `~/.dux/config.toml` (macOS)
- Session metadata stored alongside the config directory
- Per-session git worktrees with Docker-style branch names

## Install

Download the latest binary for your platform from the [GitHub Releases](https://github.com/patrickdappollonio/dux/releases) page. Extract the archive and place the `dux` binary somewhere on your `PATH` (e.g. `/usr/local/bin`).

On first launch, `dux` creates the config file with the full default configuration and comments.

## Provider Setup

The provider commands in `config.toml` point to the CLI tools you want dux to run. By default, `claude` and `codex` are configured. dux launches the configured command in a PTY inside the session's worktree directory, so the CLI tool sees the worktree as its working directory.

To use a different CLI tool, set the `command` field in the `[providers.<name>]` section of your config.

## Logging

`dux` writes runtime logs under the config directory. Log settings live in the `[logging]` section of the config file.

- `level = "error" | "info" | "debug"`
- `path = "dux.log"` uses a path relative to the config directory
- absolute paths also work if you want the log elsewhere
