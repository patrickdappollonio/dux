# dux

<img src="assets/dux-logo.png" width="200" align="right" />

Your AI agents deserve a proper office. **dux** (pronounced "dooks") is a terminal UI that lets you run multiple AI coding agents side by side, each in its own git worktree, with full companion terminals, macros, commit generation, and a command palette that knows more tricks than you do.

No protocol layers. No adapters. No JSON-RPC. Just real CLIs running in real terminals.

dux is fast and keeps resource usage low, leaving more RAM for Claude, Codex, Gemini, OpenCode, or any other agent CLI you bring.

## Why dux?

Most AI coding tools give you one agent in one directory. dux gives you multiple agents across multiple worktrees, all visible at once. Spawn agents on separate branches, let them work in parallel, fork a session to try a different approach, and open companion terminals next to your agents for builds, tests, or manual inspection.

Every agent runs through a PTY, the same pseudo-terminal your shell uses. Your MCP servers, hooks, skills, slash commands, and permission dialogs keep working because dux runs the real CLI tools you already use.

## Install with npm

Run dux directly with npx:

```bash
npx -y @patrickdappollonio/dux
```

Or install it globally:

```bash
npm install -g @patrickdappollonio/dux
dux
```

## Prerequisites

- **`git`** is required because dux is built around git worktrees.
- **`gh` CLI** is optional. If authenticated, dux can pull PR statuses and check details inside the interface.

## Documentation

The full README, release downloads, Homebrew install instructions, shell installer, screenshots, and configuration documentation live in the GitHub repository:

https://github.com/patrickdappollonio/dux
