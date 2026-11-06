---
title: Command line reference
description: Every dux command, subcommand, argument and flag, with the same help text --help prints.
group: Reference
order: 90
editSource: crates/dux/src/commands.rs
---

<!-- Generated from the dux command line's own definitions; do not edit by hand. -->

Every command dux has, with the same text `dux <command> --help` prints. For how the
commands fit together, and examples, see [The command line](/docs/command-line).

## Flags every command takes

| Flag | Value | Description |
|---|---|---|
| `--remote` | `<NAME>` | Talk to this saved remote instead of the local dux. |
| `--local` |  | Talk to the dux on this machine, whatever a saved default says. |
| `-h`, `--help` | | Print the help for the command. |
| `-V`, `--version` | | Print the version; `dux` alone takes it. |

## Environment variables

| Variable | Description |
|---|---|
| `DUX_HOME` | Override the config directory (must be an absolute path). When unset, defaults to:<br>macOS: \~/.dux/<br>Linux: $XDG_CONFIG_HOME/dux/ or \~/.config/dux/ |
| `DUX_REMOTE` | The saved remote to talk to when neither `--remote` nor `--local` is given. It wins over the default set with `dux remote default`. |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | the change failed, or what was named does not exist |
| `2` | the command line is wrong |
| `3` | refused: someone is attached, another change is in the way, or the change was not confirmed |
| `4` | dux isn't running, or does not answer |
| `5` | the remote asks for its password; sign in with `dux remote login` |
| `6` | the outcome is unknown: the wait ran out, and the change keeps running |

## `dux`

Terminal and web UI for AI worktree sessions, and a command line for their state. Run with no command to launch the terminal UI.

```text
dux [OPTIONS] [COMMAND]
```

- [`dux server`](#dux-server): Serve the web UI over the headless engine, or inspect a running server
- [`dux config`](#dux-config): Manage the dux configuration file
- [`dux projects`](#dux-projects): Projects dux knows about
- [`dux agents`](#dux-agents): Agents, their tabs and their worktrees
- [`dux terminals`](#dux-terminals): Terminals, project-owned, agent-owned and standalone
- [`dux macros`](#dux-macros): Macros from config.toml
- [`dux providers`](#dux-providers): Providers from config.toml
- [`dux keys`](#dux-keys): Keybindings from config.toml
- [`dux themes`](#dux-themes): Themes dux can load
- [`dux env`](#dux-env): The global environment from config.toml
- [`dux remote`](#dux-remote): Saved remote duxes and the sign-ins for them
- [`dux operations`](#dux-operations): Look up a change that was started earlier

## `dux server`

Serve the web UI over the headless engine, or inspect a running server.

Run the dux web UI over the headless engine. The web UI has one optional
password for one owner, and everyone who gets in shares the one workspace. Set
it with `dux config set server.auth.password`; [server.auth] require decides
who is asked for it (by default everyone but this machine and your tailnet).
With no password, anyone who can reach a non-loopback address controls your
agents and terminals, and dux says so loudly as it starts.

```text
dux server [OPTIONS] [COMMAND]
```

| Flag | Value | Description |
|---|---|---|
| `--bind` | `<ADDR:PORT>` | Bind this exact address, overriding [server] host and port. An IP:port socket address (hostnames are NOT resolved), e.g. 0.0.0.0:3890. |
| `--port` | `<PORT>` | Override [server] port only (ignored when `--bind` is set). dux binds host:port, and the machine's Tailscale address unless disabled. |
| `--no-tailscale` |  | Skip Tailscale detection this run (serve the configured host only). |

- [`dux server logs`](#dux-server-logs): Show the server's log: the last lines of server.log, read from the file when dux is not running
- [`dux server connections`](#dux-server-connections): The connections attached to the server

### `dux server logs`

Show the server's log: the last lines of server.log, read from the file when dux is not running.

```text
dux server logs [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `-f`, `--follow` |  | Keep printing new lines as they are written, until interrupted. |
| `--lines` | `<N>` | How many of the last lines to start from. Default: `100`. |

### `dux server connections`

The connections attached to the server.

```text
dux server connections [OPTIONS] <COMMAND>
```

- [`dux server connections ls`](#dux-server-connections-ls): List the connections

### `dux server connections ls`

Also: `list`

List the connections.

```text
dux server connections ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

## `dux config`

Manage the dux configuration file.

```text
dux config <COMMAND>
```

- [`dux config path`](#dux-config-path): Print the config file path
- [`dux config get`](#dux-config-get): Print one setting's value, for example `dux config get server.port`
- [`dux config set`](#dux-config-set): Change one setting, keeping the file's comments, and have a running dux reload, saying whether it worked (exit 1 if it refused or half applied, 6 if it gave no answer in time)
- [`dux config diff`](#dux-config-diff): Show settings that differ from defaults (summary; [env] and project details are summarized, never printed, so it is safe to paste into a bug report)
- [`dux config reset`](#dux-config-reset): Remove config, logs and saved remotes (keeps agents and worktrees)
- [`dux config regenerate`](#dux-config-regenerate): Preview a fresh default config (shows diff; [env] and other sensitive values are hidden unless you add `--show`)
- [`dux config restore-docs`](#dux-config-restore-docs): Preview re-adding the explanatory comments to your config, keeping every value you have set (hides values like regenerate; `--show` reveals them)

### `dux config path`

Print the config file path.

```text
dux config path
```

### `dux config get`

Print one setting's value, for example `dux config get server.port`.

```text
dux config get [OPTIONS] <SETTING>
```

| Argument | Description |
|---|---|
| `<SETTING>` | Required. The setting's dotted name, for example `server.port`. |

| Flag | Value | Description |
|---|---|---|
| `--show` |  | Values that can hold secrets (env, projects) need `--show`. |

### `dux config set`

Change one setting, keeping the file's comments, and have a running dux reload, saying whether it worked (exit 1 if it refused or half applied, 6 if it gave no answer in time)

`dux config set server.auth.password` sets the web UI password: asked for twice without echo, with a strength meter. Never a command-line argument; add `--stdin` to pipe it in instead. env.\<NAME\> values are asked for the same way.

```text
dux config set [OPTIONS] <SETTING> [VALUE]
```

| Argument | Description |
|---|---|
| `<SETTING>` | Required. The setting's dotted name, for example `server.port`. |
| `<VALUE>` | Optional. The new value. Lists are one TOML array: `'["a", "b"]'`. A value that starts with two dashes goes after `--`. Left out for the password and env.\<NAME\> values, which are asked for. |

| Flag | Value | Description |
|---|---|---|
| `--stdin` |  | Pipe the password or an env.\<NAME\> value in instead of being asked for it. |

### `dux config diff`

Show settings that differ from defaults (summary; [env] and project details are summarized, never printed, so it is safe to paste into a bug report).

```text
dux config diff [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--raw` |  | Show a unified diff against the default config. This prints the WHOLE config, [env] values included: redact it before sharing. |

### `dux config reset`

Remove config, logs and saved remotes (keeps agents and worktrees).

```text
dux config reset [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--all` |  | Full factory reset: remove config, logs, saved remotes, sessions, and worktrees (deletes nothing if a program dux started still runs). |

### `dux config regenerate`

Preview a fresh default config (shows diff; [env] and other sensitive values are hidden unless you add `--show`).

```text
dux config regenerate [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Overwrite the config file with fresh defaults. |
| `--show` |  | Show [env] and other sensitive values in the preview. |

### `dux config restore-docs`

Preview re-adding the explanatory comments to your config, keeping every value you have set (hides values like regenerate; `--show` reveals them).

```text
dux config restore-docs [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Apply it (writes a timestamped backup first). |
| `--show` |  | Show the hidden values in the preview. |

## `dux projects`

Projects dux knows about.

```text
dux projects [OPTIONS] <COMMAND>
```

- [`dux projects ls`](#dux-projects-ls): List projects
- [`dux projects show`](#dux-projects-show): Show one project
- [`dux projects add`](#dux-projects-add): Add a project from a path and print its id last
- [`dux projects rm`](#dux-projects-rm): Remove a project. Its agents' worktrees stay on disk unless `--delete-worktrees` is given
- [`dux projects worktrees`](#dux-projects-worktrees): The worktrees of a project

### `dux projects ls`

Also: `list`

List projects.

```text
dux projects ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux projects show`

Show one project.

```text
dux projects show [OPTIONS] <PROJECT>
```

| Argument | Description |
|---|---|
| `<PROJECT>` | Required. A project's id or name; a name two projects share is refused with both ids. |

### `dux projects add`

Add a project from a path and print its id last.

```text
dux projects add [OPTIONS] <PATH>
```

| Argument | Description |
|---|---|
| `<PATH>` | Required. The folder of the repository to add, on the machine dux runs on. |

| Flag | Value | Description |
|---|---|---|
| `--name` | `<NAME>` | The project's name (the folder's name by default). |
| `--checkout-default` |  | Check the repository's default branch out first. |
| `--init` |  | Make a plain folder a git repository first, with a first commit. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux projects rm`

Also: `remove`

Remove a project. Its agents' worktrees stay on disk unless `--delete-worktrees` is given.

```text
dux projects rm [OPTIONS] <PROJECT>
```

| Argument | Description |
|---|---|
| `<PROJECT>` | Required. A project's id or name; a name two projects share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--delete-worktrees` |  | Also delete its agents and their worktrees. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |
| `--dangerously-ignore-connected` |  | Go ahead even though someone is attached. |

### `dux projects worktrees`

The worktrees of a project.

```text
dux projects worktrees [OPTIONS] <COMMAND>
```

- [`dux projects worktrees ls`](#dux-projects-worktrees-ls): List a project's worktrees

### `dux projects worktrees ls`

Also: `list`

List a project's worktrees.

```text
dux projects worktrees ls [OPTIONS] <PROJECT>
```

| Argument | Description |
|---|---|
| `<PROJECT>` | Required. A project's id or name; a name two projects share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

## `dux agents`

Agents, their tabs and their worktrees.

```text
dux agents [OPTIONS] <COMMAND>
```

- [`dux agents ls`](#dux-agents-ls): List agents
- [`dux agents show`](#dux-agents-show): Show one agent
- [`dux agents add`](#dux-agents-add): Create an agent and print its id last
- [`dux agents rm`](#dux-agents-rm): Delete an agent. With no branch flag, only the branch dux made for the agent goes with the worktree, and only while dux can prove it is still that branch; every other branch is kept
- [`dux agents stop`](#dux-agents-stop): Stop everything an agent runs
- [`dux agents start`](#dux-agents-start): Start an agent
- [`dux agents tabs`](#dux-agents-tabs): An agent's provider tabs

### `dux agents ls`

Also: `list`

List agents.

```text
dux agents ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--project` | `<PROJECT>` | Only the agents of this project. |
| `--worktrees` |  | Print only each agent's id and the folder it works in. |
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux agents show`

Show one agent.

```text
dux agents show [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |

### `dux agents add`

Create an agent and print its id last.

```text
dux agents add [OPTIONS] <--project <PROJECT>|--fork <AGENT>|--standalone <FOLDER>|--from-pr <PR>>
```

| Flag | Value | Description |
|---|---|---|
| `--project` | `<PROJECT>` | The project to create the agent in, on a new branch unless `--from-pr` or `--from-worktree` says otherwise. Not with `--fork` or `--standalone`. |
| `--name` | `<NAME>` | The agent's name, which is also its branch's name in a project. |
| `--existing-branch` |  | Create the agent on the branch of that name that already exists. Not with `--from-pr`, `--from-worktree`, `--fork` or `--standalone`. |
| `--from-pr` | `<PR>` | Create the agent from this pull request: a number or its address with `--project`, or an address or owner/repo#123 alone, which clones the repository (or adds a checkout of it) when no project has it. Not with `--from-worktree`, `--fork` or `--standalone`. |
| `--path` | `<FOLDER>` | The folder to clone the pull request's repository into, or the existing checkout of it to add as a project, when no project has it. By default the server's project-browser start folder plus the repository's name. Needs `--from-pr`, and not with `--project`. |
| `--review-copy` |  | Agree to a separate review copy of the pull request when its own branch cannot be used as it is (it has diverged, is checked out elsewhere, or could not be compared). Without this such a create is refused and nothing is made, because pushes from a copy land on the copy's branch and not on the pull request. Needs `--from-pr`. |
| `--from-worktree` | `<PATH>` | Create the agent on this worktree the project already has. Needs `--project`. |
| `--standalone` | `<FOLDER>` | Create a standalone agent in this folder of your own, with no project. Not with `--fork`. |
| `--provider` | `<NAME>` | The provider a standalone agent runs (the global default otherwise). Not with `--project`, `--fork` or `--from-pr`. |
| `--fork` | `<AGENT>` | Fork this agent: a new worktree from its branch. |
| `--copy-uncommitted` |  | Copy the uncommitted changes of the project's checkout into the new worktree. Not with `--from-pr`, `--from-worktree`, `--fork` or `--standalone`. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux agents rm`

Also: `remove`

Delete an agent. With no branch flag, only the branch dux made for the agent goes with the worktree, and only while dux can prove it is still that branch; every other branch is kept.

```text
dux agents rm [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--delete-worktree` |  | Also remove the agent's worktree. |
| `--delete-branch` |  | Delete the branch its worktree is on, at the commit it points at now. Needs `--delete-worktree`: git will not delete a branch a worktree still has checked out. Not with `--keep-branch`. |
| `--keep-branch` |  | Keep the branch its worktree is on. With `--delete-worktree`, the other branches it has been on are still decided as the delete dialog would tick them, unless `--keep-other-branches` or `--delete-other-branches` says otherwise. |
| `--delete-other-branches` |  | Delete every other branch its worktree has been on, each at the commit it points at now. Needs `--delete-worktree`, and not with `--keep-other-branches`. |
| `--keep-other-branches` |  | Keep every other branch its worktree has been on. With `--delete-worktree`, the branch it is on now is still decided as the delete dialog would tick it, unless `--keep-branch` or `--delete-branch` says otherwise. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |
| `--dangerously-ignore-connected` |  | Go ahead even though someone is attached. |

### `dux agents stop`

Stop everything an agent runs.

```text
dux agents stop [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |
| `--dangerously-ignore-connected` |  | Go ahead even though someone is attached. |

### `dux agents start`

Start an agent.

```text
dux agents start [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux agents tabs`

An agent's provider tabs.

```text
dux agents tabs [OPTIONS] <COMMAND>
```

- [`dux agents tabs ls`](#dux-agents-tabs-ls): List an agent's tabs
- [`dux agents tabs add`](#dux-agents-tabs-add): Add a tab to an agent
- [`dux agents tabs rm`](#dux-agents-tabs-rm): Close a tab
- [`dux agents tabs start`](#dux-agents-tabs-start): Start a tab
- [`dux agents tabs stop`](#dux-agents-tabs-stop): Stop a tab

### `dux agents tabs ls`

Also: `list`

List an agent's tabs.

```text
dux agents tabs ls [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux agents tabs add`

Add a tab to an agent.

```text
dux agents tabs add [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--provider` | `<NAME>` | The provider the tab runs (the project's by default). |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux agents tabs rm`

Also: `remove`

Close a tab.

```text
dux agents tabs rm [OPTIONS] <AGENT> <TAB>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |
| `<TAB>` | Required. A tab's id or the label the tab strip shows it under. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |
| `--dangerously-ignore-connected` |  | Go ahead even though someone is attached. |

### `dux agents tabs start`

Start a tab.

```text
dux agents tabs start [OPTIONS] <AGENT> <TAB>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |
| `<TAB>` | Required. A tab's id or the label the tab strip shows it under. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux agents tabs stop`

Stop a tab.

```text
dux agents tabs stop [OPTIONS] <AGENT> <TAB>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Required. An agent's id or name; a name two agents share is refused with both ids. |
| `<TAB>` | Required. A tab's id or the label the tab strip shows it under. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |
| `--dangerously-ignore-connected` |  | Go ahead even though someone is attached. |

## `dux terminals`

Terminals, project-owned, agent-owned and standalone.

```text
dux terminals [OPTIONS] <COMMAND>
```

- [`dux terminals ls`](#dux-terminals-ls): List terminals
- [`dux terminals add`](#dux-terminals-add): Open a terminal and print its id last
- [`dux terminals rm`](#dux-terminals-rm): Close a terminal

### `dux terminals ls`

Also: `list`

List terminals.

```text
dux terminals ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux terminals add`

Open a terminal and print its id last.

```text
dux terminals add [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--agent` | `<AGENT>` | Open it in this agent's worktree. Not with `--project`. |
| `--project` | `<PROJECT>` | Open it at this project's root. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux terminals rm`

Also: `remove`

Close a terminal.

```text
dux terminals rm [OPTIONS] <TERMINAL>
```

| Argument | Description |
|---|---|
| `<TERMINAL>` | Required. A terminal's id or its name in the list; a name two terminals share is refused with both ids. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |
| `--dangerously-ignore-connected` |  | Go ahead even though someone is attached. |

## `dux macros`

Macros from config.toml.

```text
dux macros [OPTIONS] <COMMAND>
```

- [`dux macros ls`](#dux-macros-ls): List the macros
- [`dux macros show`](#dux-macros-show): Show one macro by name
- [`dux macros add`](#dux-macros-add): Add a macro, or replace the one with that name
- [`dux macros rm`](#dux-macros-rm): Remove a macro

### `dux macros ls`

Also: `list`

List the macros.

```text
dux macros ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux macros show`

Show one macro by name.

```text
dux macros show [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The macro's name, as `dux macros ls` lists it. |

### `dux macros add`

Add a macro, or replace the one with that name.

```text
dux macros add [OPTIONS] <NAME> <TEXT>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The macro's name; an existing macro with this name is replaced. |
| `<TEXT>` | Required. The text the macro types. |

| Flag | Value | Description |
|---|---|---|
| `--surface` | `<SURFACE>` | Where the macro is offered: in agents, terminals, or both. One of `agent`, `terminal`, `both`. Default: `agent`. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux macros rm`

Also: `remove`

Remove a macro.

```text
dux macros rm [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The macro's name, as `dux macros ls` lists it. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

## `dux providers`

Providers from config.toml.

```text
dux providers [OPTIONS] <COMMAND>
```

- [`dux providers ls`](#dux-providers-ls): List the providers, built-in and your own
- [`dux providers show`](#dux-providers-show): Show one provider and the command it runs

### `dux providers ls`

Also: `list`

List the providers, built-in and your own.

```text
dux providers ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux providers show`

Show one provider and the command it runs.

```text
dux providers show [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The provider's name, as `dux providers ls` lists it. |

## `dux keys`

Keybindings from config.toml.

```text
dux keys [OPTIONS] <COMMAND>
```

- [`dux keys ls`](#dux-keys-ls): List every action the terminal UI binds and the keys it uses, yours or the default

### `dux keys ls`

Also: `list`

List every action the terminal UI binds and the keys it uses, yours or the default.

```text
dux keys ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

## `dux themes`

Themes dux can load.

```text
dux themes [OPTIONS] <COMMAND>
```

- [`dux themes ls`](#dux-themes-ls): List the themes dux can load, marking the one ui.theme names

### `dux themes ls`

Also: `list`

List the themes dux can load, marking the one ui.theme names.

```text
dux themes ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

## `dux env`

The global environment from config.toml.

```text
dux env [OPTIONS] <COMMAND>
```

- [`dux env ls`](#dux-env-ls): List the variables
- [`dux env set`](#dux-env-set): Set a variable; the value is asked for, or read from stdin
- [`dux env rm`](#dux-env-rm): Remove a variable

### `dux env ls`

Also: `list`

List the variables.

```text
dux env ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--show` |  | Print the values too, which may be secrets. |
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux env set`

Set a variable; the value is asked for, or read from stdin.

```text
dux env set [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The variable's name, for example `GITHUB_TOKEN`. |

| Flag | Value | Description |
|---|---|---|
| `--stdin` |  | Read the value from standard input. |
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

### `dux env rm`

Also: `remove`

Remove a variable.

```text
dux env rm [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The variable's name, as `dux env ls` lists it. |

| Flag | Value | Description |
|---|---|---|
| `--yes` |  | Do not ask for confirmation (required without a terminal). |
| `--no-wait` |  | Print the operation id and return without waiting for the change. |
| `--wait-timeout` | `<SECONDS>` | Wait at most this many seconds for the change to finish (at most a day). Not with `--no-wait`. |

## `dux remote`

Saved remote duxes and the sign-ins for them.

```text
dux remote [OPTIONS] <COMMAND>
```

- [`dux remote add`](#dux-remote-add): Save a remote under a name
- [`dux remote ls`](#dux-remote-ls): List saved remotes
- [`dux remote rm`](#dux-remote-rm): Forget a saved remote
- [`dux remote default`](#dux-remote-default): Choose, or with `--unset` clear, the remote used when none is named
- [`dux remote login`](#dux-remote-login): Sign in to a remote (the selected one when no name is given)
- [`dux remote logout`](#dux-remote-logout): Sign out of a remote (the selected one when no name is given)

### `dux remote add`

Save a remote under a name.

```text
dux remote add [OPTIONS] <NAME> <URL>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The name to save the remote under, used with `--remote`. |
| `<URL>` | Required. The address of the remote dux's web UI, for example `https://box.tailnet-name.ts.net`. |

| Flag | Value | Description |
|---|---|---|
| `--insecure` |  | Allow plain http:// to an address that is neither this machine nor a Tailscale address. The password then crosses the network unencrypted, and every login says so. |

### `dux remote ls`

Also: `list`

List saved remotes.

```text
dux remote ls [OPTIONS]
```

| Flag | Value | Description |
|---|---|---|
| `--format` | `<FORMAT>` | How to print the list: `table` to read, `json` for a script. One of `table`, `json`. Default: `table`. |
| `-q`, `--quiet` |  | Print only the ids, one per line. |

### `dux remote rm`

Also: `remove`

Forget a saved remote.

```text
dux remote rm [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Required. The saved remote's name, as `dux remote ls` lists it. |

### `dux remote default`

Choose, or with `--unset` clear, the remote used when none is named.

```text
dux remote default [OPTIONS] [NAME]
```

| Argument | Description |
|---|---|
| `<NAME>` | Optional. The saved remote's name, as `dux remote ls` lists it. Needed unless `--unset` is given. |

| Flag | Value | Description |
|---|---|---|
| `--unset` |  | Clear the default remote. Not with a name. |

### `dux remote login`

Sign in to a remote (the selected one when no name is given).

```text
dux remote login [OPTIONS] [NAME]
```

| Argument | Description |
|---|---|
| `<NAME>` | Optional. The saved remote's name (the selected one when left out). |

| Flag | Value | Description |
|---|---|---|
| `--stdin` |  | Read the password from standard input instead of asking for it. |

### `dux remote logout`

Sign out of a remote (the selected one when no name is given).

```text
dux remote logout [OPTIONS] [NAME]
```

| Argument | Description |
|---|---|
| `<NAME>` | Optional. The saved remote's name (the selected one when left out). |

## `dux operations`

Look up a change that was started earlier.

```text
dux operations [OPTIONS] <COMMAND>
```

- [`dux operations show`](#dux-operations-show): Show where a change stands

### `dux operations show`

Show where a change stands.

```text
dux operations show [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` | Required. The operation id a change printed when it started. |
