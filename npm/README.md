# dux

<img src="assets/dux-logo.png" width="200" align="right" />

Your AI agents deserve a proper office. **dux** (pronounced "dooks") runs multiple AI coding agents side by side, each in its own git worktree, with companion terminals, provider tabs, macros, and a full git staging area. Drive it from a terminal, or run `dux server` and drive the same workspace from a browser, phone included.

No protocol layers. No adapters. No JSON-RPC. Just real CLIs running in real terminals.

dux is fast and keeps resource usage low, leaving more RAM for Claude, Codex, Copilot, OpenCode, or any other agent CLI you bring.

## Why dux?

Most AI coding tools give you one agent in one directory. dux gives you multiple agents across multiple worktrees, all visible at once. Spawn agents on separate branches, let them work in parallel, fork a session to try a different approach, run several provider tabs inside one agent so two CLIs share a checkout, and open companion terminals next to your agents for builds, tests, or manual inspection.

Every agent runs through a PTY, the same pseudo-terminal your shell uses. Your MCP servers, hooks, skills, slash commands, and permission dialogs keep working because dux runs the real CLI tools you already use.

## Two front ends, one engine

dux has two front ends over one engine: a terminal UI and a web UI. Both are first class, and both are staying. They share the same projects, the same agents, the same worktrees and the same config file, so an agent you start in one is the same agent in the other.

They are not identical, on purpose. Each surface does what its medium is good at. The terminal gives you full keyboard control, rebindable keys, a command palette and themes. The browser gives you reach: any device on your network, including a phone, plus editing files in the page and desktop notifications. Where a capability only makes sense on one side, it lives on one side. The app itself is the reference for which is which: the terminal's help overlay and command palette, the browser's cog menu and row menus.

## Server mode, and the fact that it has no login

`dux server` serves the web UI over the same workspace: the same `config.toml`, the same projects, the same agents on the same worktrees, the same live PTYs. Nothing is mirrored or re-synced. By default it binds `127.0.0.1:8080`, loopback only, and if the `tailscale` CLI is around it also binds this machine's Tailscale address so your own tailnet devices can reach it.

Before you serve it anywhere: **there is no login.** No password, no token, no user accounts. dux is a single-tenant, trusted-access tool and server mode is honest about that. Everyone who can reach the address shares one workspace: they can drive any agent or terminal, browse the server's filesystem, edit files in your worktrees, and see every session. That is deliberate, so access control is entirely a question of where you bind.

The safe shapes are loopback (the default), your own tailnet, or a reverse proxy you put in front and authenticate yourself, which is also where TLS would live since dux serves plain HTTP. A LAN or public address is not a safe shape: it puts your agents and worktrees in reach of anyone who can hit it, and the loud warning dux prints is the only thing standing there. Don't serve it to anyone you wouldn't hand a shell on that machine.

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
