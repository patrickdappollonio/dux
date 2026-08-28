---
title: Startup commands & environment variables
description: How to run per-project setup scripts and inject environment variables into every agent dux creates.
group: Guides
order: 30
---

Some projects need ceremony before an agent is useful: pulling in dependencies,
symlinking secrets files, warming a cache. Two config features cover it, both inside
`[[projects]]` entries in `config.toml`: per-project **environment variables** and
**startup commands**.

## Per-project environment variables

The `env` field on a project is an inline TOML table of `KEY = "value"` pairs. dux
passes them to everything it spawns for that project: agent sessions, companion
terminals, and the startup command.

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/api"
name = "api"
env  = { NODE_ENV = "development", API_KEY = "${MY_API_KEY}" }
```

> [!TIP]
> Values expand `$VAR` and `${VAR}` from your shell environment when dux starts. Keep
> secrets as references rather than literals and `config.toml` stays safe to commit to
> your dotfiles.

You do not have to edit the file to change them. In the terminal UI, *configure project
env* opens the project's variables one per line:

![The terminal UI project environment editor, showing three KEY=value lines for a project with Cancel and Save buttons below them.](/screens/tui-project-env-editor.png)

### Global environment variables

A top-level `[env]` table applies to every project. Project-level `env` keys override
global ones, so a global `LOG_LEVEL = "info"` can be bumped to `"debug"` for one project
without touching the rest.

```toml
[env]
LOG_LEVEL = "info"
EDITOR    = "true"

[[projects]]
id   = "a4f3..."
path = "$HOME/projects/api"
name = "api"
env  = { LOG_LEVEL = "debug" }   # overrides the global LOG_LEVEL for this project
```

## Startup commands

`startup_command` is a string, or a multiline TOML string, that runs inside the agent's
worktree right after that worktree is created and **before** the provider launches. Use
it for anything the agent needs already done: installing packages, symlinking config
files, running code generators.

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
startup_command = """
npm ci
ln -sfn "$DUX_PROJECT_PATH/.env.local" .env
"""
```

- It runs with its working directory set to the **agent's worktree**, not the source
  checkout.
- dux waits for it to finish before launching the provider.
- Every run writes a timestamped log under the dux config directory, at
  `startup-command-logs/<project-id>/<session-id>/`. Browse them from the command
  palette in the terminal UI or a row's actions menu on the web, for one agent or for
  every agent in a project at once.

> [!IMPORTANT]
> A startup command that exits non-zero does not block you. dux records the failure in
> the startup log and launches the agent anyway, so check the log when an agent starts
> without its dependencies.

Each log names the run, the command, the shell it ran through, how long it took, its exit
code, and everything it printed. This is the terminal UI's viewer:

![The terminal UI startup command log viewer, listing one run beside its output: the command, exit code, and the lines it printed while preparing the worktree.](/screens/tui-startup-command-log.png)

### Configuring and running from the app

You can edit both by hand, but you do not have to leave the app:

- **Terminal UI:** the command palette carries *configure startup command*, *configure
  project env*, *configure global env*, *rerun startup command on agent*, and *read
  startup command logs*.
- **Web UI:** each agent's `⋯` menu carries *Configure startup command*, *Configure
  environment variables*, *Rerun startup command*, and *Startup command logs*. A
  project's `⋯` menu carries *Startup command logs for all agents*. Global env lives in
  the cog menu's Configuration submenu.

*Rerun startup command* re-runs the command in one agent's worktree without recreating
the agent. Reach for it after editing the command, or when a dependency install needs a
redo.

> [!NOTE]
> Env and startup commands are project-scoped, so editing them from an agent changes the
> agent's whole project, and the change is written back to `config.toml`. None of these
> four entries appear for a standalone agent, which belongs to no project.

### Dux-injected variables

dux sets these for every startup command, on top of any `[env]` and `[[projects]] env`
keys you configure:

| Variable | Value |
|---|---|
| `DUX_PROJECT_PATH` | Absolute path to the project's source checkout |
| `DUX_WORKTREE_PATH` | Absolute path to the agent's git worktree |
| `DUX_AGENT_ID` | UUID that uniquely identifies this agent session |
| `DUX_AGENT_BRANCH` | Git branch name for this agent's worktree |
| `DUX_PROVIDER` | Provider name used for this agent (e.g. `claude`, `codex`) |
| `DUX_STARTUP_COMMAND_LOG` | Absolute path to the log file for this run |

> [!IMPORTANT]
> The `DUX_*` variables exist **only** for startup commands. Agent sessions and
> companion terminals do not get them. They inherit dux's own environment, plus the
> `TERM`, `COLORTERM` and terminal-identity values dux sets, with your `[env]` and
> `[[projects]] env` keys layered on top.

A standalone terminal belongs to no project, so it gets the global `[env]` and nothing
else, and it runs no startup command. Neither does a project terminal: a startup command
is worktree provisioning for a new agent, not a shell rc.

## The startup shell

Startup commands run through a shell. The global `[startup_command_terminal]` section
picks which one:

```toml
[startup_command_terminal]
# Shell used to run project startup commands before launching a new agent.
# "$SHELL" is expanded when the command runs and falls back to /bin/sh if unset.
command = "$SHELL"
# Arguments passed before the configured project startup command.
# The default ["-l", "-c"] runs a login shell without interactive job-control warnings.
args = ["-l", "-c"]
```

The defaults run your login shell as `$SHELL -l -c "<your startup_command>"`, so your
shell profile, `$PATH` extensions, and version managers like `nvm`, `rbenv` or `mise`
are active. The section is global, so every project and every machine you sync the
config to behaves the same. To pin a specific shell:

```toml
[startup_command_terminal]
command = "/opt/homebrew/bin/bash"
args    = ["-l", "-c"]
```

## Practical examples

### Node.js project with a secrets file

```toml
[[projects]]
id   = "b8c2..."
path = "$HOME/projects/frontend"
name = "frontend"
env  = { NODE_ENV = "development" }
startup_command = """
npm ci
ln -sfn "$DUX_PROJECT_PATH/.env.local" .env
"""
```

`npm ci` runs inside the worktree so each agent gets its own `node_modules`, and the
symlink points at the source checkout's `.env.local` so all agents share one copy of the
local secrets.

### Python project with a virtual environment

```toml
[[projects]]
id   = "d1e9..."
path = "$HOME/projects/backend"
name = "backend"
env  = { VIRTUAL_ENV = "$HOME/projects/backend/.venv", API_TOKEN = "${BACKEND_API_TOKEN}" }
startup_command = """
python -m venv .venv
.venv/bin/pip install -q -r requirements.txt
"""
```

Each worktree gets its own `.venv` so agents cannot step on each other's packages.

### Cargo workspace with pre-built tools

```toml
[[projects]]
id   = "f3a7..."
path = "$HOME/projects/cli-tool"
name = "cli-tool"
startup_command = "cargo build -q 2>&1 | tail -5"
```

Builds the workspace quietly so the agent's first edit-compile-test loop is faster.
`2>&1 | tail -5` keeps only the last five lines in the log.
