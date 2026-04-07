# dux

`dux` is a terminal UI for managing AI coding sessions per git worktree.

## How It Works

`dux` spawns AI CLI tools (`claude`, `codex`, `opencode`, `gemini`, or any terminal command) directly in a pseudo-terminal (PTY) and renders their output in real time. There is no protocol layer, no adapter binaries, and no JSON-RPC — just the official CLI running exactly as it would in your terminal.

This means:

- **Bring any CLI.** Any AI coding tool that runs in a terminal works with dux. Configure the command and args in `config.toml` and you're set.
- **No banning risks.** You're running the official CLI the way it was designed to be used — the same binary, the same auth, the same API calls.
- **Full CLI feature support.** Hooks, MCP servers, skills, slash commands, permission dialogs, thinking indicators — everything the CLI supports works out of the box because dux is just hosting the real terminal session.
- **Crash recovery.** If dux crashes, the agent process dies, but dux can reconnect in the same worktree. Providers with configured `resume_args` (like the built-in Claude, Codex, OpenCode, and Gemini defaults) can resume the CLI conversation for that folder/worktree.

## Features

- Left pane for projects and worktree sessions
- Fork an existing agent session into a new worktree with the current files copied over
- Center pane for live agent terminal output or file diffs
- Right pane for changed files and diffs
- Resizable panes with keyboard shortcuts and mouse drag
- Collapsible project sidebar and git pane
- Command palette with fuzzy search
- Config written to `~/.config/dux/config.toml` (Linux) or `~/.dux/config.toml` (macOS)
- Session metadata stored alongside the config directory
- Per-session git worktrees with Docker-style branch names

## Install

Download the latest binary for your platform from the [GitHub Releases](https://github.com/patrickdappollonio/dux/releases) page. Extract the archive and place the `dux` binary somewhere on your `PATH` (e.g. `/usr/local/bin`).

On first launch, `dux` creates the config file with the full default configuration and comments.

## Config Management

`dux config` provides subcommands for inspecting and managing the configuration file:

- `dux config path` — print the config file path.
- `dux config diff` — show settings that differ from defaults (summary view).
- `dux config diff --raw` — show a unified diff against the default config.
- `dux config reset` — remove config and logs so dux can recover from a broken or outdated configuration while keeping saved agents and their worktrees intact.
- `dux config reset --all` — full factory reset: also remove `sessions.sqlite3` and the managed `worktrees/` directory.
- `dux config regenerate` — preview a fresh default config (shows diff against current).
- `dux config regenerate --yes` — overwrite the config file with fresh defaults.

## Provider Setup

The provider commands in `config.toml` point to the CLI tools you want dux to run. By default, `claude`, `codex`, `opencode`, and `gemini` are configured, and new sessions start with `claude` unless you override it per project or in `[defaults]`. dux launches the configured command in a PTY inside the session's worktree directory, so the CLI tool sees the worktree as its working directory.

To use a different CLI tool, set the `command` field in the `[providers.<name>]` section of your config.

If your CLI supports resuming the most recent session for the current repository/folder, add `resume_args` for reconnects after a crash or detached session. If `resume_args` is omitted or empty, dux assumes that CLI does **not** support session resume and will relaunch it normally.

```toml
[providers.example]
command = "example-agent"
args = []
resume_args = ["resume", "--last"]
```

## Logging

`dux` writes runtime logs under the config directory. Log settings live in the `[logging]` section of the config file.

- `level = "error" | "info" | "debug"`
- `path = "dux.log"` uses a path relative to the config directory
- absolute paths also work if you want the log elsewhere

## Keybindings

All keybindings are configured in the `[keys]` section of `config.toml`. On first launch, every binding is written out with its default value and a description comment.

Key format: single characters (`"j"`), special names (`"enter"`, `"space"`, `"pageup"`, `"shift-tab"`, `"esc"`), or modifier combos (`"ctrl-d"`, `"ctrl-p"`).

Each action maps to an array of key combos. For example, to rebind quit from `q`/`ctrl-c` to just `ctrl-q`:

```toml
[keys]
quit = ["ctrl-q"]
```

The `show_terminal_keys` option controls whether hints for terminal-native keys (like `ctrl-j` for newline) appear in the UI. These keys work regardless of this setting — dux documents them but does not control them.

```toml
[keys]
show_terminal_keys = false
```

Text input keys (Backspace, typing characters, Enter in the commit editor) and PTY passthrough keys in interactive mode are not rebindable.

Invalid key strings cause the app to refuse to start with a clear error pointing to the broken entry.
