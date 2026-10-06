---
title: The command line
description: List and change projects, agents, tabs, terminals, macros and the rest from a shell, on this machine or a dux somewhere else, with output a script can read and exit codes it can trust.
group: Guides
order: 55
---

dux is a terminal UI, a web UI, and a command line. Everything you can see in the sidebar
you can list from a shell, and most of what you can do to it you can do from a script:

```bash
dux projects ls
dux agents add --project web --name fix-login
dux agents tabs add fix-login --provider codex
dux agents rm fix-login --delete-worktree
dux macros add Review "review this diff for bugs" --surface agent
```

The commands are nouns with verbs under them, the way `docker` does it: `ls` (also
`list`), `show`, `add`, `rm` (also `remove`), and `start` and `stop` where they make
sense. `--help` works at every level, from `dux --help` down to `dux agents tabs rm --help`.

## It talks to a running dux

Projects, agents, tabs, worktrees, terminals and connections live in the dux that is
running, so the command line asks that dux rather than reading anything behind its back.
Any dux will do: the terminal UI, the web server you flipped to, the terminal UI serving
in the background, or `dux server`. Nothing needs turning on.

With no dux running, those commands stop and say so:

```text
dux isn't running; start it with "dux" or "dux server"
```

A dux that is running but cannot be reached says that instead, with its PID, and why
when dux knows. The usual fix is to restart it.

The resources that live in `config.toml` (macros, providers, keys, themes and the global
environment) do not need dux running; [see below](#resources-kept-in-configtoml).

## Commands

| Command | What it does |
|---|---|
| `dux projects ls`, `show <project>` | Projects, with their branch and how many agents each has. |
| `dux projects add <path>` | Adds a project. `--name` names it, `--checkout-default` checks the repository's default branch out first, and `--init` makes a plain folder a git repository with a first commit. |
| `dux projects rm <project>` | Removes a project. Its agents' worktrees stay on disk unless you add `--delete-worktrees`. |
| `dux projects worktrees ls <project>` | Every worktree the project has, which agent holds it, and whether it is dirty, in use, or being removed. |
| `dux agents ls`, `show <agent>` | Agents, with their project or folder, provider, state, running tabs and how many browsers are connected. `--project` keeps one project's; `--worktrees` prints just each agent's id and the folder it works in. |
| `dux agents add` | Creates an agent. `--project` on a new branch, plus `--existing-branch`, `--from-pr <number or URL>` or `--from-worktree <path>`; `--fork <agent>`; or `--standalone <folder>` with an optional `--provider`. `--name` names it and `--copy-uncommitted` brings the project's uncommitted changes along. |
| `dux agents rm <agent>` | Deletes an agent. `--delete-worktree` removes its worktree too, and `--delete-branch` or `--keep-branch` decides the branch; with neither, a branch dux created goes and one it found is kept, as in the delete dialogs. `--delete-branch` needs `--delete-worktree`, because git will not delete a branch a worktree still has checked out. |
| `dux agents stop <agent>`, `start <agent>` | Stops everything an agent runs, or starts it again. |
| `dux agents tabs ls <agent>` | The agent's provider tabs and whether each is running. |
| `dux agents tabs add <agent>` | Adds a tab, running the project's provider unless `--provider` names another. |
| `dux agents tabs rm`, `start`, `stop` `<agent> <tab>` | Closes, starts or stops one tab. Stopping keeps the tab, closing removes it. |
| `dux terminals ls` | Every terminal and who owns it. |
| `dux terminals add` | Opens a terminal: in an agent's worktree with `--agent`, at a project's root with `--project`, or a standalone one in your home folder with neither. |
| `dux terminals rm <terminal>` | Closes a terminal. |
| `dux macros ls`, `show`, `add`, `rm` | Macros. `add <name> <text>` takes `--surface agent`, `terminal` or `both` (`agent` by default) and replaces a macro of the same name. |
| `dux providers ls`, `show <name>` | Providers, built-in and your own. |
| `dux keys ls` | The terminal UI's keybindings. |
| `dux themes ls` | The themes dux can load, and which one is in use. |
| `dux env ls`, `set <name>`, `rm <name>` | The global environment. |
| `dux server logs`, `dux server connections ls` | The server's log and who is connected; see [Server mode](/docs/server-mode#the-servers-own-log). |
| `dux remote …` | Other machines' duxes; see [below](#another-machines-dux). |
| `dux operations show <id>` | Where a change started earlier stands. |

A project, agent, tab, terminal or macro is named by its id or by its name. A name that
two of them share is refused with both ids, so you can pick one. A new project, agent,
tab or terminal's id is the last line `add` prints, which makes it easy to capture:

```bash
agent=$(dux agents add --project web --name fix-login --yes | tail -n 1)
```

Paths you give are made absolute for the dux on this machine. A remote dux is another
machine, so it refuses a relative path; give the whole thing.

## Output

Every `ls` prints an aligned table with a header row. Two flags change that:

- `--format json` prints one JSON array of objects, with more fields than the table has
  room for.
- `-q` prints only the ids, one per line, ready for `xargs`.

`dux agents ls`, for example, has the columns `ID`, `NAME`, `PROJECT`, `WORKTREE`,
`PROVIDER`, `STATE`, `TABS` (running of total) and `REMOTE` (browsers connected). An
agent's state is `active`, `detached` or `exited`, or `removing` while its deletion is
still running. `show` prints one field per line.

## Changes ask first, and wait until they are done

A change asks before it does anything, and the question names the dux it is about to
change:

```text
Delete agent fix-login on this machine's dux? [y/N]
```

`--yes` answers for you. Without a terminal to ask on (a script, a pipe, cron), a change
with no `--yes` is refused and nothing happens.

Then it waits until the change has really finished, not just started, and prints how it
went: a deleted agent's worktree is gone and its branch is deleted or kept before the
command returns. If part of a change failed, the command says which part and exits 1.

A second `dux agents add` that runs while another agent is still being created waits for
that creation to finish (within the same wait time), then creates its own and says it
waited. If the wait time runs out before the other creation finishes, it never starts its
own: it is refused and exits 3. With `--no-wait` it is refused at once, as the browser and
the terminal UI refuse it.

- `--no-wait` prints the change's id and returns at once.
- `--wait-timeout <seconds>` waits a different time than usual. The usual time is
  `[cli] wait_timeout_seconds` (10 minutes). No wait lasts more than a day; see
  [Configuration](/docs/configuration#how-long-a-command-waits-cli).
- A wait that runs out says the outcome is unknown, prints the id, and exits 6. The change
  has not been stopped: it carries on, and `dux operations show <id>` tells you how it
  ends.

dux keeps a finished change's outcome for `[server] operation_retention_seconds` (30
minutes by default), in memory, so a restart forgets them all.

One thing changes at a time. Changing an agent, tab, terminal or project that another
change is still working on, from anywhere, is refused with what is in the way.

## Nobody gets cut off by accident

Deleting or stopping an agent, closing or stopping a tab, closing a terminal
and removing a project are refused while somebody else is connected to what they would
end: a browser watching the agent, or the terminal UI showing it. The refusal lists them:

```text
Someone else is using this right now:
  a browser at 100.101.102.103, watching tab <tab id>
Nothing was changed. Ask them to close it first, or add --dangerously-ignore-connected to go ahead and cut them off.
```

`--dangerously-ignore-connected` goes ahead anyway. `--yes` never does: it answers the
question, it does not decide that cutting someone off is fine. A phone that went to sleep
on the agent keeps counting as connected for `[server] presence_grace_seconds` (5 minutes
by default), so the agent you are working on from the couch stays protected.

> [!IMPORTANT]
> The command line has no screen of its own, so it is never the one connected. If the
> terminal UI is showing the agent you are deleting from a shell, the terminal UI is in the
> way, and the command is refused until you add the flag.

## Resources kept in config.toml

Macros, providers, keys, themes and the global environment live in `config.toml`, so on
this machine their listings read the file itself, whether dux is running or not.

Changes (`dux macros add` and `rm`, `dux env set` and `rm`) go through the running dux
when there is one, so they apply at once, in the terminal UI and in every browser. With
no dux running they edit the file, keeping its comments, and apply the next time dux
starts. Providers, keys and themes are read-only here: change a provider's fields with
`dux config set`, and keys and themes in the terminal UI or the file.

The environment's values are usually secrets, so they are treated like it:

- `dux env ls` prints only the names. `--show` prints the values too.
- `dux env set <name>` asks for the value without echoing it, or reads it from a pipe with
  `--stdin`. It never takes the value as an argument, where shell history would keep it.
  Reading from a pipe leaves no terminal to confirm on, so `--stdin` always needs `--yes`.

A macro whose name is anything other than letters, digits, `_` and `-` keeps its text
hidden, the same rule `dux config get` follows: `ls` lists it under a placeholder name, and
`dux macros show '<name>'` finds it by its real name but prints only that placeholder and
its surface. The macro editor in the browser shows it in full.

## Another machine's dux

A dux serving the web UI on another machine can be driven from here too. Save it once:

```bash
dux remote add box https://box.tailnet-name.ts.net
dux remote login box        # only if it asks for a password
dux --remote box agents ls
```

| Command | What it does |
|---|---|
| `dux remote add <name> <url>` | Saves a remote. |
| `dux remote ls` | The saved remotes, which one is the default, and which you are signed in to. |
| `dux remote rm <name>` | Forgets a remote and drops its sign-in here, without telling the remote; that sign-in then ends there once it goes unused. `logout` first to end it at once. |
| `dux remote default <name>`, `default --unset` | Picks the remote used when none is named, or goes back to this machine. |
| `dux remote login [<name>]`, `logout [<name>]` | Signs in with that dux's [web password](/docs/web-login), or out. |

The commands that talk to a dux take two flags, before or after the command's own words,
and `dux config` takes them before the word `config`. Starting dux or `dux server` takes
neither:

- `--remote <name>` talks to that saved remote.
- `--local` talks to this machine's dux, whatever else says otherwise.

Which dux a command talks to is decided in this order: `--local`, then `--remote`, then the
`DUX_REMOTE` environment variable, then the default remote, then this machine. A
`DUX_REMOTE` set in your shell profile works as set-and-forget. Whatever is chosen is
used: a remote that does not answer stops the command, and it never quietly tries this
machine instead.

> [!WARNING]
> `dux config` always edits this machine's `config.toml`. With a remote selected, through
> `DUX_REMOTE` or a default, it refuses rather than let you think you changed the other
> machine. Run `dux --local config …` to go ahead.

The listings of macros, providers, keys, themes and the environment come from the remote's
own config when a remote is selected, and so do the changes to macros and the
environment.

### Signing in

A remote that does not ask this machine for a password needs no sign-in: commands reach it
as they are. That covers a dux on your tailnet with the default `require = "network"`, and
any dux with no password at all. One that does ask stops with exit 5 when this machine needs
a sign-in there (none saved, or the saved one ended):

```text
box asks for its password; run "dux remote login box"
```

`dux remote login` asks for the password without echoing it, or reads it from a pipe with
`--stdin`. Wrong guesses count exactly like wrong guesses on the sign-in page, slow-downs
and bans included. A sign-in stays good until it goes unused for
`[server.auth] cli_token_idle_days` on that dux (30 by default), until you run
`dux remote logout`, or until the password changes, which signs every command line out at
once.

Saved remotes and their sign-ins are kept in `remotes.toml` in the config folder, readable
only by you, never in `config.toml`. Neither file is safe to paste into a bug report:
`config.toml` holds your environment's secrets and the password hash. Paste the plain
`dux config diff` summary instead.
`dux config reset` removes it.

### Plain HTTP

`https://` works everywhere. Plain `http://` is accepted only to this machine and to
Tailscale addresses and `.ts.net` names, where the connection is already encrypted. Any
other plain-HTTP address is refused, because your password would cross the network in the
clear; add `--insecure` to `dux remote add` if you trust every network in between, and
every login to it then says so.

The remote has to accept the name you reach it by. A name it does not know is refused by
that dux before anything else; add it to its `[server] allowed_hosts`.

## Exit codes

Scripts can rely on these:

| Code | Meaning |
|---|---|
| `0` | It worked. |
| `1` | The change failed or partly failed, or what you named does not exist. |
| `2` | The command line is wrong: a bad flag, a name two things share, an unknown remote. |
| `3` | Refused: somebody is connected, another change is in the way, or the change was not confirmed. |
| `4` | dux isn't running, or does not answer. |
| `5` | The remote asks for its password and this machine needs a sign-in there (none saved, or the saved one ended). |
| `6` | The outcome is unknown: the wait ran out, and the change keeps running. |

`dux operations show <id>` exits 0 for a change that succeeded, 1 for one that failed or
partly failed, and 6 for one still running.
