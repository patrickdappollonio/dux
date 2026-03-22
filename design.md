# dux Design

## Goal

`dux` is a terminal-first orchestrator for AI coding agents, inspired by multi-agent tools like Claude Squad, but centered around:

- one git worktree per agent
- a persistent TUI for managing projects and sessions
- a live agent pane plus a diff/shell review workflow
- ACP-based provider integration where possible

## Product Model

### Projects

A project is a git repository registered in user config.

Each project has:

- absolute path
- display name
- default provider
- source branch context

Projects appear in the left pane and act as the root for agent sessions.

### Agent Sessions

Each agent session:

- starts from a project
- creates a dedicated git branch
- creates a dedicated git worktree
- launches one provider process for that worktree
- persists metadata in SQLite

Sessions render under their project in the left tree.

### Center Pane

The center pane is intentionally dual-purpose:

- live agent output
- diff viewer

The user should be able to inspect diffs without killing the running agent session.

### Right Pane

The right pane is split:

- top: changed files list
- bottom: manual shell rooted in the selected worktree

The top section is read-only review. Git actions can happen in the shell or through the agent.

## Current Architecture

### Runtime

- Rust
- `ratatui` + `crossterm` for TUI
- `portable-pty` for manual shell sessions
- `rusqlite` for persisted session state
- ACP over stdio for agent providers

### Main Modules

- [src/app.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/app.rs): TUI state and workflow
- [src/acp.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/acp.rs): ACP transport and request/notification handling
- [src/git.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/git.rs): git and worktree operations
- [src/storage.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/storage.rs): SQLite persistence
- [src/config.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/config.rs): user config schema and rendering
- [src/statusline.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/statusline.rs): UI status model
- [src/logger.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/logger.rs): log file integration

## UX Model

### Navigation

- `Tab` and `Shift-Tab` move across panes
- `Esc` closes the topmost overlay
- overlays are the consistent home for modal interaction

### Action Entry

The app supports both:

- key combos
- command-driven interaction

Command-driven interaction now matters because:

- terminal apps and shells can compete for keybindings
- users should not need to memorize many shortcuts
- future actions can be added without crowding the keyboard map

Current command entry modes:

- `Ctrl-P`: floating command palette
- `:`: direct command mode

### Status Presentation

The footer is split into two lines:

- key-hint line
- status line

The status line supports:

- info state
- busy state with spinner
- error state

## Config and State

### Config

Location:

- `~/.config/dux/config.toml`

Properties:

- fully materialized
- comment-rich
- hand-editable
- includes defaults, providers, UI settings, logging, and projects

### Session State

Location:

- `~/.config/dux/sessions.sqlite3`

Stores:

- session metadata
- worktree path
- provider
- ACP session id
- status
- timestamps

### Logging

Location:

- default: `~/.config/dux/dux.log`

Configurable in `[logging]`:

- `level = error|info|debug`
- `path`

## Behavior Rules

### Project Refresh

- runs in the original checkout
- blocks if the source checkout is dirty
- uses `git pull --ff-only`

### Agent Creation

- runs asynchronously
- shows progress in the status line
- creates worktree first
- launches provider second
- performs ACP initialize and `session/new`
- fails fast on provider/ACP timeout
- cleans up the new worktree on startup failure

### Session Restore

On startup, persisted sessions are loaded and:

- worktree existence is checked
- shell sessions are recreated when possible
- ACP sessions are reloaded when an ACP session id exists and reload succeeds
- otherwise the session is marked detached

## Intended Direction

These remain good follow-on improvements:

- real provider capability detection and validation in config UI
- richer command palette actions and fuzzy search
- explicit project editing/rename support
- better project deletion UX with confirmation overlay
- more robust session restore and history handling
- mouse-based pane resizing
- better diff rendering with scroll position and syntax/color awareness
- separate module extraction from `src/app.rs` as UI complexity grows

## Design Recommendations

- Prefer explicit failure states over silent waiting.
- Keep the UI responsive by pushing blocking work onto workers.
- Treat the command palette as the main extensibility surface for UX.
- Preserve the split between project config and runtime state.
- Preserve the fully rendered config file; it is part of the product UX, not just implementation detail.
