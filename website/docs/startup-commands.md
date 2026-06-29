---
title: Startup commands & environment variables
description: How to run per-project setup scripts and inject environment variables into every agent dux creates.
group: Guides
order: 30
---

Some projects need a little ceremony before an agent is actually useful: pulling
in dependencies, symlinking secrets files, or warming a cache. dux handles this
with two complementary config features: per-project **environment variables** and
**startup commands**. Both live inside `[[projects]]` entries in `config.toml`
and run before the provider launches, so every agent your team creates starts
from the same clean slate.

## Per-project environment variables

The `env` field on a project is an inline TOML table of `KEY = "value"` pairs.
dux passes these variables to every PTY it spawns for that project: agent
sessions, companion terminals, and the startup command itself.

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/api"
name = "api"
env  = { NODE_ENV = "development", API_KEY = "${MY_API_KEY}" }
```

Values expand `$VAR` and `${VAR}` from your shell environment at the moment dux
starts. That means secrets stay as references (never hardcoded in the file),
which makes `config.toml` safe to commit to your dotfiles.

### Global environment variables

A top-level `[env]` table applies to every project. Project-level `env` keys
override global ones when both are set, so a global `LOG_LEVEL = "info"` can be
bumped to `"debug"` for one project without touching the rest.

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

`startup_command` is a string (or multiline TOML string) that runs inside the
agent's worktree immediately after that worktree is created, **before** the
provider launches. It is the right place for anything the agent needs already
done when it first opens: installing packages, symlinking config files, running
code generators.

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

A few things to know:

- The command runs with its working directory set to the **agent's worktree**
  (i.e. `$DUX_WORKTREE_PATH`), not the source checkout.
- dux waits for the command to complete before launching the provider. If the
  command exits non-zero, dux records the failure in the startup log and still
  launches the agent; it does not block you.
- Every run produces a timestamped log file under the dux config directory:
  `startup-command-logs/<project-id>/<session-id>/`. You can browse these from
  the command palette (TUI) or an agent's actions menu (web UI).

### Configuring and running from the app

Both `env` and `startup_command` are ordinary config you can edit by hand, but
you do not have to leave the app to manage them:

- **Terminal UI:** the command palette exposes *configure startup command*,
  *configure project env*, *configure global env*, *rerun startup command on
  agent*, and *read startup command logs*.
- **Web UI (server mode):** each agent's actions (`⋯`) menu carries
  *Configure startup command*, *Configure environment variables*, *Rerun
  startup command*, and *Startup command logs*. Because env and startup commands
  are project-scoped, the first two edit the agent's whole project (and the
  change is written back to `config.toml`); global env stays in the command
  palette. *Rerun startup command* re-runs the project's startup command in that
  one agent's worktree without recreating it, which is handy after editing the
  command or when a dependency install needs a redo.

### Dux-injected variables

dux sets the following environment variables for every startup command, in
addition to any `[env]` and `[[projects]] env` keys you configure:

| Variable | Value |
|---|---|
| `DUX_PROJECT_PATH` | Absolute path to the project's source checkout |
| `DUX_WORKTREE_PATH` | Absolute path to the agent's git worktree |
| `DUX_AGENT_ID` | UUID that uniquely identifies this agent session |
| `DUX_AGENT_BRANCH` | Git branch name for this agent's worktree |
| `DUX_PROVIDER` | Provider name used for this agent (e.g. `claude`, `codex`) |
| `DUX_STARTUP_COMMAND_LOG` | Absolute path to the log file for this run |

These variables are available exclusively inside startup commands. Agent PTY
sessions and companion terminals receive only your configured `[env]` and
`[[projects]] env` variables.

## The startup shell

Startup commands run through a shell, not directly. The global
`[startup_command_terminal]` section controls which shell and arguments to use:

```toml
[startup_command_terminal]
# Shell used to run project startup commands before launching a new agent.
# "$SHELL" is expanded when the command runs and falls back to /bin/sh if unset.
command = "$SHELL"
# Arguments passed before the startup command text.
# The default ["-l", "-c"] runs a login shell without interactive job-control warnings.
args = ["-l", "-c"]
```

The defaults run your login shell (`$SHELL`) with `-l -c`, so your shell
profile, `$PATH` extensions, and tool version managers (e.g. `nvm`, `rbenv`,
`mise`) are active when the command runs. The effective invocation looks like:

```
$SHELL -l -c "<your startup_command>"
```

Because `[startup_command_terminal]` is global config (not project state), the
shell behavior is the same for every project and every machine you sync the
config to. Change it once and all startup commands pick it up.

If you need a specific shell for a particular environment, point `command` at it
directly:

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

`npm ci` runs inside the worktree so each agent gets its own `node_modules`.
The symlink points back at the project source checkout's `.env.local` so all
agents share the same local secrets without duplicating them.

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

Each worktree gets its own `.venv` so agents can't step on each other's
installed packages.

### Cargo workspace with pre-built tools

```toml
[[projects]]
id   = "f3a7..."
path = "$HOME/projects/cli-tool"
name = "cli-tool"
startup_command = "cargo build -q 2>&1 | tail -5"
```

Builds the workspace quietly so the agent's first edit-compile-test loop is
faster. The `2>&1 | tail -5` keeps the log compact: only the last five lines
of build output are captured.
