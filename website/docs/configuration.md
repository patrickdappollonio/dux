---
title: Configuration
description: Where the config file lives, how it expands environment variables, and the commands that manage it.
group: Getting started
order: 2
---

dux follows one rule above all others: **the config file is the documentation.** Every
setting is configurable, and every setting is commented inline. You should never have to
leave `config.toml` to understand what an option does.

## Where it lives

dux writes a fully annotated `config.toml` the first time it launches:

- **Linux:** `~/.config/dux/config.toml`
- **macOS:** `~/.dux/config.toml`

Themes are preselected, keybindings are ready to remap, and the default providers are
already wired in. Open it, read the comments, change what you like.

The remotes the [command line](/docs/command-line#another-machines-dux) can talk to, and
its sign-ins to them, are kept beside it in `remotes.toml`, readable only by you. They
never go in `config.toml`.

Each provider block carries its own settings, including `web_dragdrop_paste`, which
decides what a dragged, dropped or pasted file's path looks like when the web UI writes it
into that agent's prompt. dux ships a measured value for every CLI it knows about, so you
do not normally set it. See [Custom agents and providers](/docs/custom-agents) for the
anatomy of a provider block, and
[Dropping and pasting files onto an agent](/docs/dropping-files) for which CLI wants which
value.

## Managing the config

A handful of subcommands handle the file without you hunting for it:

- `dux config path` prints the absolute path to the active config file.
- `dux config get <setting>` prints one setting's value (`--show` prints one that can
  hold secrets).
- `dux config set <setting> <value>` changes one setting and has a running dux reload it,
  then says whether the reload worked. For a secret, leave the value out to be asked for it, or pipe it in with
  `--stdin`.
- `dux config diff` shows what you have changed from the defaults.
- `dux config regenerate` previews the latest canonical template, so you can see new
  options after an upgrade.
- `dux config restore-docs` puts the explanatory comments back into a config that lost
  them, keeping every value exactly as it is.
- `dux config reset` removes the config, the log, the server log with its rotated copies
  and your saved remotes, and keeps your agents and worktrees.
  `dux config reset --all` is the full factory reset: all of that, the session database and
  every worktree dux made. It deletes nothing if a program dux started is still running,
  and it keeps one thing, the note that you have already seen the welcome screen, in the
  fresh database it leaves behind, so the next start does not greet you as a stranger.
  Both refuse while another dux is running.

Hand-edits are preserved across saves: your comments and ordering survive.

`dux config` always works on this machine's file. When the command line has a remote
selected, through `DUX_REMOTE` or a default remote, it refuses rather than let you think
you changed the other machine; `dux --local config …` goes ahead.

### Reading and changing one setting

A setting is named by its table and key, joined with dots:

```bash
dux config get server.port               # 3890
dux config set server.port 4000
dux config set ui.theme "catppuccin_mocha"
dux config set server.allowed_hosts '["dux.example.com", "box.example.com"]'
dux config set providers.claude.command claude
```

- `get` prints what `config.toml` says, or, when the file leaves the setting out, the value
  dux actually uses with that file (a note on stderr says so, so the value alone is what a
  script captures). Where dux uses something other than what the file says (a log level
  it does not know runs as `info`, a value past its ceiling runs at the ceiling, a `0`
  that means the default runs as the default), `get` prints what dux uses, and the note
  names what the file says and why. A provider listed without a command reads as an
  empty command, and `get` says that provider cannot start. It also works when dux
  refuses to start with the file, which is how you look at the broken part: it prints
  what the file says and names the problem that keeps dux from working out the value it
  would use.
- `set` never writes a file dux would refuse to start with: a value that would add such a
  problem (a host name where `server.host` needs an IP address, an environment variable
  name dux does not accept) is refused with the same message a start gives, and nothing
  is written. So is a value dux would not use as written when it loads the file: one it
  would drop, put back to its default or hold at a limit (a terminal font size past its
  range, a `gh` re-check interval below its floor, a theme the terminal UI has no
  built-in or file for, a provider changed back into a retired stock block dux removes).
  The refusal names the value dux would use instead.
  A setting written inside a section or entry the file already breaks some other way is
  taken, and `set` says it stays at its default until that problem is fixed.
- A setting with a value of the wrong type (a word where a number goes) is read two ways:
  `dux server` uses that setting's default and starts, while the terminal UI will not start
  until it is fixed. `get` and `set` say so in those words, and `get` says the value in use
  cannot be worked out until then.
- If `config.toml` is not valid TOML at all, `get` and `set` refuse it too and name the
  line that is wrong. Fix that line by hand (or copy back a backup), then carry on.
- `set` checks the value before writing it: a number has to be a number in range, a
  setting with a fixed set of values has to be one of them, and a list is written whole as
  one TOML array. Only that one line of the file changes; every comment stays.
- `[server.auth]` settings are also checked against the rest of the section as the file
  has it, so a `minimum_password_length` that does not fit your own `max_password_bytes`
  is refused. When the section already has problems, `set` still fixes them one at a
  time: a change is refused only if it adds a problem, and after each change `set` lists
  what is still wrong, since dux will not start until all of it is fixed.
- Providers are named by their name (`providers.<name>.<field>`), and environment
  variables by theirs (`env.<NAME>`). Changing one field of a built-in provider your file
  does not list writes that provider out in full with your change, so it keeps working. A
  new name adds a provider, starting with its command: `set providers.<name>.command`
  first, and `set` refuses anything that would leave a provider without a command.
- Environment values are treated as secrets, since that is where API tokens live.
  `dux config set env.GITHUB_TOKEN` asks for the value without echoing it (or reads it,
  up to 1 MiB, from a pipe with `--stdin`) and never takes it as an argument, and it
  reports only that the value was updated. `get` on `env`, an `env.<NAME>` value or `projects` says whether
  it is set but prints the value only when you add `--show`.
- The web password is `server.auth.password`, a setting with no line of its own: `set`
  asks for it (or reads `--stdin`), checks it against your minimums, and stores only its
  hash. See [the web password](#the-web-password-serverauth).
- A value that itself starts with two dashes goes after `--`, which ends the flags:
  `dux config set server.title -- "--staging--"`. A single dash, as in a negative number, needs
  nothing.
- A misspelled setting is refused with the closest real one. The message repeats only the
  part of the name that exists, never what you typed after it, in case that was a value
  typed where a name goes: `server has no setting below it with that name; did you mean
  server.port? Values are never given in the path.`
- `[[projects]]`, `[keys]` and `[macros]` are read with `get` but not written with `set`,
  because each has rules of its own. Edit them in the file, or use dux itself.
- A key whose own name contains a dot cannot be named this way. Edit the file for that
  one.

After writing, `set` asks the dux running on this machine to reload its config and waits
for the answer. It then says which of these happened:

- The reload applied: the new settings are in force. If a changed setting is only read when
  dux starts or when a server binds, the same message says so and what to restart.
- The reload was refused: the change is saved in `config.toml` but not in force, because the
  running dux could not take the file on, and the message gives the reason. The running dux
  keeps its previous settings. `set` exits with status 1 after writing; fix the file and run
  a command that reloads again, or use the reload command in dux.
- The reload applied but one step of applying it failed: the new settings are in force, the
  message names the step that failed and says to fix it and reload again, and `set` exits
  with status 1. The same goes when the change also started, stopped or moved a listener
  (the background web server, the Tailscale address) and that could not be done: `set`
  waits for the listener to answer, and the message names what it refused or why it failed.
- dux is not running: nothing was asked. `set` prints "dux is not running, so the change
  applies the next time it starts." and exits with status 0.
- dux is running but could not be asked: the change is saved, and `set` prints "The change is
  saved, but the running dux was not asked to reload it:" followed by why (for example that
  dux is running without a control socket, or does not answer on it), then says dux keeps its
  current settings until you reload it from the app or restart it. It exits with status 0.
- dux did not answer in time: `set` waits up to `[cli] wait_timeout_seconds` (10 minutes by
  default) for the reload's answer. If none comes it prints "The change is saved, but the
  running dux has not said whether the reload worked, so its outcome is unknown:" followed by the operation it was
  following, which keeps running and can be looked up later with `dux operations show`, and
  exits with status 6, the command line's code for an outcome that is not known.

The running dux also reports how the reload went in its status line, in the web UI's
notifications and in `dux.log`; two `set`s in quick succession are both picked up. If dux
is not running, the change applies the next time it starts.

Writers take turns on the file, and a save dux makes from its own settings only writes the
settings that changed inside dux since it last read or saved the file, keeping the comments
around them. So a value you `set`, or edit or delete by hand, while dux runs stays as you
left it across any number of saves, unless you then change that same setting inside dux
before it reloads. The same goes for each field of a project. Projects are matched by their
id and path together, then by id, then by path, so a project you add to the file by hand
stays when dux adds one of its own, and a project written without an id is given one the
first time dux saves. A setting the file has never had (one new in this version of dux) is
written in with its current value. Once dux reloads the file, a setting missing from it
counts as one the file has never had, so the next save writes it in with its default. The
`[server.auth]` section is only ever changed by an explicit change to it (`set`, a password
change, a block after failed logins), never by dux saving its other settings.

> [!TIP]
> After editing `config.toml` by hand, you can ask a running dux to reload it without
> touching the app: `kill -USR1 <pid>`. The PID is the first line of `dux.lock`, next to the config.

> [!WARNING]
> A factory reset deletes every worktree dux made, uncommitted work included. It refuses to
> run while another dux is running, and it is all or nothing. First it stops what dux itself
> started and left running in those worktrees (a startup command's dev server, or a job a
> terminal left working in one, say), giving each the same grace period as a delete before
> forcing it. That includes everything a standalone agent started, wherever it works. Then it
> looks once more: if anything dux started still runs, or any program of yours is working in a
> folder the reset would delete (one dux never started included), or dux cannot tell whether
> something does (it cannot read the database, the list of what it started, or the process
> list), the reset deletes nothing: not the config, not the database, not a worktree. It says
> why, lists each program and its folder, and exits with status 1. Stop what is listed and run
> the reset again. If a folder cannot be removed once the deleting has begun, the reset stops
> there, keeps the database and the config, lists what it removed and what failed, and exits
> with status 1; fix that and run it again. A folder a standalone agent runs in is never
> removed. Once the reset does delete, it keeps
> the record that you have seen the welcome screen, so the screen does not open again on
> the next start.

### What `dux config diff` shows, and what it holds back

The summary compares your file as written against the built-in defaults, so it reports
what you typed rather than what dux normalizes it into: no clamping of out-of-range
numbers, and no default provider block quietly folded in. A setting added in a new release
turns up in your diff the day it ships, with no list for anyone to maintain.

Two things are summarized rather than printed:

- `[env]` reports only `env: changed`. Those values are frequently API tokens, and a
  token in a terminal scrollback is a token you have to rotate.
- `[[projects]]` reports only a count, because projects carry their own `env` and a
  project's index is not a stable name.

Macros report a count too, since a macro body is arbitrary prose. The web UI password's
hash reports only `server.auth.password_hash: changed`, and `blocked_addresses` only a
count: the first lets anyone who has it try guesses at your password, and the second is a
list of other people's IP addresses. Everything else is shown as
`setting: default -> yours`, with long values cut off at 40 characters.

> [!CAUTION]
> The plain summary is safe to paste into a bug report. **`dux config diff --raw` is
> not.** It prints a unified diff of your entire config, including every value in your
> `[env]` table verbatim. Redact it before you share it anywhere.

### Getting the comments back

Ordinary saves preserve comments rather than adding them, so a `config.toml` that never
had any stays bare forever. `dux config restore-docs` fixes that:

```bash
dux config restore-docs        # preview: shows a diff, changes nothing
dux config restore-docs --yes  # apply
```

It is careful with your data:

- Every value survives: projects and their ids, macros with multi-line bodies, provider
  commands and their arguments, and environment values.
- Applying it writes a timestamped backup first and prints where it went.
- Settings dux does not recognize are kept as they are, not quietly deleted, and are
  listed in the output.
- A few sections dux genuinely no longer reads are removed, and the removal is reported.
- The preview holds back sensitive values: an `[env]` value, a project's values, the
  web UI's `password_hash` and `blocked_addresses`, a key binding dux cannot read and
  anything below a setting it does not recognize show as their name and
  `(not shown)`, and comments inside `[env]` and
  `[projects]` (including one at the end of a header line) are hidden too. Add `--show`
  to see them. A plaintext web UI password is never shown, `--show` or not.
  `dux config regenerate` previews the same way, and when your file is not valid TOML
  its preview shows none of it, `--show` included, and names the line to fix instead.
- If the file cannot be parsed, the command refuses and changes nothing rather than
  falling back to defaults, which would throw your settings away.

## The web password (`[server.auth]`)

The web UI's optional password and everything around it. How to set it, who is asked for
it, and what it does and does not protect against are in
[The web password](/docs/web-login); this is the reference.

```bash
dux config set server.auth.password                                 # asks twice, rates it as you type
printf '%s\n' "$PW" | dux config set server.auth.password --stdin   # for scripts
dux config set server.auth.password_hash ""                         # remove the password
```

`server.auth.password` is the one setting `set` never takes as an argument, where shell
history and the process list would keep it. The prompt never echoes what you type.
`--stdin` drops one trailing line break, the one `printf '%s\n'` or a file's last line
adds, and refuses to read from a terminal, where what you typed would show. A password
cannot contain a line break, a tab or any other control character, because a browser's
password field cannot type one: a pipe holding one more line break (a file ending in a
blank line) is refused, and nothing is changed. What `set` stores is the password's
Argon2id hash at `password_hash`, and `get` on either key prints that hash. Changing or
clearing it signs every browser out. While the file has a password setting where dux does
not read it (a `minimum-password-length` spelled with dashes, say), setting a password is
refused until that is fixed: dux cannot tell which minimum you meant.

> [!IMPORTANT]
> dux reads the password only from `[server.auth]`. A `password_hash` anywhere else, or a
> table with a near-miss name such as `[server.auht]` or `[server.Auth]`, stops dux from
> starting (and a running dux keeps its settings on reload) instead of letting it run
> without the password you meant to set. The message names where the key is and where it
> belongs.

| Setting | Default | What it does |
| --- | --- | --- |
| `password_hash` | `""` | The password's Argon2id hash, or empty for no password. Set it with the command above rather than by hand. |
| `require` | `"network"` | Who is asked for the password: `"network"` (everyone but this machine and your tailnet), `"tailnet"` (everyone but this machine) or `"everywhere"`. Use `"everywhere"` behind a reverse proxy. |
| `minimum_password_length` | `12` | The fewest characters a new password may have. |
| `minimum_password_score` | `2` | The lowest strength a new password may have, from 0 (weak) through fair, good and strong to 4 (excellent). |
| `session_idle_seconds` | `60` | How long a browser stays signed in with nothing happening. An open dux tab counts as something happening. At least 1. |
| `cli_token_idle_days` | `30` | How many days a sign-in made from the dux command line survives without being used. Unlike a browser, the command line keeps its sign-in between commands, so daily use never asks for the password again. A password change signs every one out. At least 1. |
| `max_failed_logins` | `5` | Failed sign-ins one address may make, each within `failed_login_window_seconds` of the one before, before dux adds it to `blocked_addresses`. An IPv6 client on the network is also counted by its /64, and when the addresses of one /64 together reach the limit without any one of them reaching it alone, the whole /64 is added as one range; never for this machine's own network, link-local `fe80::` addresses, your tailnet or IPv4, which are counted one address at a time. Only an address dux can verify is added (a direct connection, or one through your `tailscale serve`); behind another proxy dux keeps slowing the sign-ins down and its log names the address for you to add by hand. `0` never blocks; the slow-down still applies. |
| `blocked_addresses` | `[]` | Addresses and CIDR ranges refused outright, with or without a password. A request matches when any address it names does, forwarding headers included. Loopback and this machine's own addresses never match: the part of an entry covering them is accepted, warned about and ignored, and the rest of a range still applies. dux adds single addresses, or an IPv6 /64 as one range that replaces the addresses of it already listed. Yours to edit. |
| `failed_login_window_seconds` | `900` | How long a failed sign-in counts against its address. |
| `failed_login_delay_seconds` | `1` | The wait after a failed sign-in or a wrong current password, doubling with each further one. This machine waits too. `0` turns it off. |
| `failed_login_max_delay_seconds` | `30` | The longest that wait grows. |
| `max_failed_logins_per_minute` | `30` | Failures from many addresses together before every visitor in that group, those that never failed included, is told to wait out the minute. Counted apart for the internet and unverified proxies, for devices on your network, and for a browser on this machine using one of its own non-loopback addresses; your tailnet devices and this machine on `localhost` are slowed one address at a time while dux can tell who they are, and otherwise sign-ins on `localhost` share one count of their own, direct tailnet devices share the network's, and `tailscale serve` visitors share the internet's. Visitors through a `tailscale serve` TCP forward look exactly like this machine and share its count. `0` turns it off. |
| `disable_no_auth_warning` | `false` | Hides the browser's red no-password banner. Its "Don't show again" button sets this. |
| `cookie_secure` | `"auto"` | Whether the sign-in cookie is marked Secure: `"auto"` (when dux knows the browser used HTTPS: a `tailscale serve` HTTPS route or a Tailscale Funnel), `"always"` or `"never"`. `"always"` also tells dux browsers reach it over HTTPS, so the sign-in page drops its plain-HTTP warning. |
| `max_concurrent_password_checks` | `2` | Password checks run at once. Each costs about 19 MiB and some CPU on purpose, so this bounds what a flood of attempts costs. At least 1. |
| `password_check_queue` | `8` | Sign-ins that may wait for a free check; more are told to try again shortly. |
| `max_password_bytes` | `1024` | The longest password accepted, in bytes, from 1 to 65536 and no smaller than `minimum_password_length`. |
| `max_tracked_addresses` | `10000` | Addresses whose failed sign-ins dux remembers at once; the stalest is forgotten first. At least 1. |
| `max_blocked_addresses` | `1000` | How many entries `blocked_addresses` may hold, your own included, before dux stops adding to it. Past it, a new automatic block holds until dux restarts; entries you add yourself are never refused. |

Every one of them applies on a reload, with no restart.

> [!IMPORTANT]
> A mistake in `[server.auth]` is never read as "no password". A misspelled key, a value
> of the wrong type, a `password_hash` dux will not use, a password setting written
> outside `[server.auth]` (`require` directly under `[server]`, a `password-hash` under
> any spelling, a `[server.authentication]` table), a plaintext `password` written
> anywhere in the file (dux keeps only the hash; set it with `dux config set
> server.auth.password`), or a `config.toml` that is not valid TOML at all stops dux
> from starting, with a message naming the file and the problem, and a reload while it
> runs changes nothing until the file is fixed.
> When the section is invalid, `dux config get` and `dux config set` keep working, so
> you can inspect and repair it from the command line. When the file is not valid TOML,
> they refuse it too; fix the line the error names by hand. `dux config set` itself
> refuses any value that would leave the section invalid. If `config.toml` is deleted
> while dux runs, a reload is refused the same way and the running settings, password
> included, stay until the file is back; `dux config set` refuses to write a fresh file
> then too, and points you at Recover config.

`dux config regenerate --yes` writes fresh defaults, which have no password; it says so
when the config it replaces had one.

## Logs (`[logging]`)

```toml
[logging]
level      = "info"    # error, warn, info or debug
path       = ""        # empty means dux.log in the config directory
max_bytes  = 10485760  # rotate once the log reaches 10 MiB; 0 never rotates
keep       = 5         # how many rotated copies to keep
compress   = true      # gzip the rotated copies
```

dux rotates its own log by size, so you do not need logrotate or a cron for it.
When the log is about to pass `max_bytes`, dux renames it to `dux.log.1`, moves
`dux.log.1` to `dux.log.2` and so on, deletes anything past `keep`, and starts a
fresh `dux.log`. With `compress = true` the rotated copies are gzipped in the
background and are named `dux.log.1.gz`, `dux.log.2.gz` and so on; read one with
`zcat` or `gunzip -c`. Set `keep = 0` to rotate and throw the old log away, and
`max_bytes = 0` to never rotate at all.

Rotation happens by size and only when dux writes a line, so there is no daily or
weekly schedule and a dux that is sitting idle never touches the files. A line is
always written whole, so the file stops just short of `max_bytes` rather than
crossing it, and a single line longer than the whole limit is still written and
briefly goes over. `keep` above 1000 is clamped, with a note in the log. If you
are upgrading with a log that is already far bigger than `max_bytes`, it is
rotated on the first line dux writes after the upgrade.

The server has a log of its own, `server.log`, with the same rotation settings under
`[server]`: see [the server's own log](/docs/server-mode#the-servers-own-log).

If `path` points at a symlink, dux follows it once at startup: the live log and
its rotated copies all live beside the file the link points at, and the link
itself is left alone.

> [!TIP]
> `tail -F ~/.config/dux/dux.log` (note the capital F) follows the log across a
> rotation. Plain `tail -f` keeps watching the rotated copy and goes quiet.

| Key | When it takes effect |
|---|---|
| `level` | On a config reload. Turn `debug` on while something is misbehaving and back off again without restarting. |
| `path` | At startup only. The log file is opened once, so a new path needs a restart. |
| `max_bytes`, `keep`, `compress` | On a config reload, applied at the next line dux writes. |

## How long a command waits (`[cli]`)

```toml
[cli]
wait_timeout_seconds = 600  # 10 minutes
```

A change made from the `dux` command line waits until it has really finished
before the command ends. `wait_timeout_seconds` is how long it waits; past that,
the command stops waiting and says the outcome is unknown, naming the change's id
so `dux operations show <id>` can look it up later. The change itself keeps
running either way. `--wait-timeout <seconds>` sets a different wait for one
command, and `--no-wait` returns at once with the id. Only the command line on
this machine reads this setting, the next time it runs.

## How long a clone may go quiet (`[git]`)

```toml
[git]
clone_stall_seconds = 300  # 5 minutes
```

When dux clones a repository for a new project, there is no limit on how long
the whole clone takes: a big repository is slow, not stuck. What dux watches
instead is git's progress. A clone that prints nothing for `clone_stall_seconds`
is stopped, and the message says so. Raise it for a remote that takes a long time
to start sending. Values below 1 count as 1. It takes effect for the next clone.

## Environment variables and portable paths

Project paths understand `$HOME`, `${HOME}`, and `~`, and environment values expand
`$VAR` and `${VAR}` from your shell:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
env  = { EDITOR = "true", API_KEY = "${FOO_KEY}" }
```

> [!TIP]
> The file holds portable intent (projects, providers, themes, keybindings) rather than
> runtime state, so it is **safe to commit to git**. Drop it in your dotfiles and it
> travels between machines without leaking your username or your secrets.

### Keeping secrets out of `[env]`

Anything you type literally into `[env]`, or a project's `env`, is exactly that: a
literal. It sits in `config.toml` on disk, it shows up in `dux config diff --raw`, and it
travels with the file into your dotfiles repo. Two ways to avoid that:

**Write nothing at all.** Every agent and terminal dux spawns inherits dux's own
environment, so a variable already exported in the shell you launched dux from is already
there. If `ANTHROPIC_API_KEY` comes out of your shell profile or a secrets agent, your
provider CLI finds it without `[env]` mentioning it. Use `[env]` for what dux has to add
or override.

**When you do need an entry, reference the variable instead of pasting the value.**

```toml
[env]
API_KEY = "${FOO_KEY}"   # resolved from the environment dux itself was launched in
```

Three details that matter:

- The lookup happens in dux's environment, not the agent's shell, so the variable must be
  exported *before* dux starts. Launch dux from a shell where it is set, or from a
  wrapper that sources your secrets first.
- If the variable is not set, dux does not fail and does not blank the value. It leaves
  the text alone, and the agent receives the literal string `${FOO_KEY}`. When a tool
  complains about a nonsense credential, check this first.
- `~` is not expanded in an env *value*. Tilde works in a project `path`; for a
  home-relative env value, write `$HOME/...`.

> [!WARNING]
> None of this is encryption. It keeps the secret out of the file, which is the part that
> gets committed, pasted into issues and synced between machines. The value still reaches
> every agent and terminal dux spawns, and dux is a trusted-access tool with no per-user
> isolation.

## Keybindings

Every keybinding dux uses is configurable under `[keys]`, and the in-app help overlay is
the authoritative reference for what is currently bound. Bindings are arrays, so an
action can answer to more than one key:

```toml
[keys]
quit         = ["q", "ctrl-c"]
open_palette = ["ctrl-p"]
```

Modifier and control-key parsing is case-insensitive: `Ctrl-g`, `ctrl-g`, and `CTRL-g` all
mean the same thing. Letter keys are lowercased, so bind an uppercase letter as its
shifted form, such as `shift-p`.

> [!IMPORTANT]
> While you type into an agent in the windowed pane, any chord bound here belongs to dux
> and never reaches the agent. Rebinding dux's side is how you free a chord the agent
> needs. The `input-debugging` palette command shows what dux receives for each keypress.
> Full story in [Introduction](/docs/introduction#the-three-panes).

Tab is the exception, because it moves between panes rather than typing. Set
`tab_reaches_agent = true` under `[ui]` to send Tab and Shift-Tab to the agent in the
windowed pane instead; `focus_next` and `focus_prev` (`Ctrl-o` and `Ctrl-y` by default)
move between panes whichever way it is set.

Rather than memorizing hotkeys, reach most actions through the terminal UI's command
palette. It is the fastest way to discover what dux can do.

The palette matches on names and descriptions. Exact phrase matches lead the list, and
anything that matched only loosely follows them, so a half-remembered phrase still finds
the command.

![The command palette with new tab typed and new-agent-tab as the first result.](/screens/tui-palette-matches.png)

## Per-project startup commands

A project's `startup_command` runs setup for you when an agent's worktree is created:

```toml
[[projects]]
id   = "a4f3..."
path = "$HOME/projects/web-app"
name = "web-app"
startup_command = """
npm install
ln -sfn "$DUX_PROJECT_PATH/.env.local" .env
"""
```

The shell that runs it is itself configurable under `[startup_command_terminal]`. For the
full treatment (per-project and global `env`, the `DUX_*` variables dux injects, and the
startup shell), see
[Startup commands & environment variables](/docs/startup-commands).

## Naming a web instance (title + favicon)

Running several dux servers? Two web-only settings under `[server]` tell their browser
tabs apart:

```toml
[server]
title   = "dux (prod)"   # the browser tab title and the in-app wordmark
favicon = "blue"         # recolors the duck favicon for this instance
```

`title` drives both the browser tab title and the brand wordmark. `favicon` is empty by
default (the original yellow duck); set it to one of the curated tints: `violet`, `blue`,
`sky`, `cyan`, `teal`, `green`, `amber`, `orange`, `red`, `pink`, or `rose`.

Both are also in the web UI's cog menu under **Preferences…**. The change is written to
`[server]` in `config.toml` and applies to every open tab immediately.

## Where dropped and pasted files go (`[ui]`)

Three web-only settings under `[ui]` decide what happens to a file you drop or paste onto
an **agent** pane:

```toml
[ui]
upload_directory         = ".dux/uploads"  # relative to the agent's worktree
upload_write_gitignore   = true            # hide the uploads from git
upload_pasted_text_chars = 4000            # longer pastes become a .txt file
```

`upload_directory` is where the file is saved, relative to that agent's worktree, created
the first time you drop something. It lives inside the worktree so the agent CLI can read
it, since several refuse to read outside their workspace, and so deleting the agent takes
the uploads with it.

> [!IMPORTANT]
> `upload_directory` must be a relative path with no `..` in it. An absolute, traversing
> or empty value falls back to `.dux/uploads` and says so once in `dux.log`.

`upload_write_gitignore` keeps a `.gitignore` containing a single `*` in that folder,
which ignores everything in it including itself, so your screenshots never turn up as
untracked files. dux rewrites it on every upload, so the file comes back if you delete it
or if the folder was created while the setting was off. Set it to `false` if you intend
to commit what you drop or paste. dux never edits a `.gitignore` you already have there,
and never writes to `.git/info/exclude`, which in a linked worktree would change what git
ignores in every other worktree at once.

`upload_pasted_text_chars` is the point at which text you PASTE into an agent stops being
typed at the prompt and becomes a document: dux saves it as a `.txt` file in the folder
above and pastes that file's path, which costs the agent's context window far less than a
wall of text. The default of 4000 is about a long page of prose, chosen so ordinary
instructions still arrive as text while a log or a diff becomes a file, and it counts
CHARACTERS, so a paste in Japanese is measured the way an English one is.

Set it to `0` to always paste text as text, or press `Ctrl+Shift+v` (`Cmd+Shift+v` on a
Mac) to bypass it for one paste. Values between 1 and 199, or above 100000, are clamped
with one warning in `dux.log`.

Dropping or pasting onto a **terminal** is unaffected by all three. That always lands in
the folder the terminal is actually in, and a terminal never turns a paste into a file.

`upload_write_gitignore` and `upload_pasted_text_chars` are rows in the web UI's
**Preferences** dialog, as *Hide dropped and pasted files from git* and *Save long pastes
as a file*. `upload_directory` is not: it is a path, and a free-text box is a poor way to
pick one. The full story is in
[Dropping and pasting files onto an agent](/docs/dropping-files).

## Editing settings from the web

The **Preferences…** dialog holds every web-adjustable setting, a curated set of
`[server]`, `[ui]`, `[capabilities]` and `[defaults]` preferences, grouped by which
surface they affect:

- **This browser (Web)**: the instance name and favicon above, plus copy-on-select,
  desktop notifications, the Changes pane default, and whether dropped and pasted files
  stay hidden from git.
- **Both surfaces**: status-message auto-clear, the attention indicator and its grace
  period, the always-show-tab-strip preference, the PR banner position, clickable
  hyperlinks, GitHub integration, whether dux binds your Tailscale address, and whether new
  agents start with a random pet name.

The Tailscale row does more than save a value: it also moves the listener that is
serving right now and tells you what happened to it. Choosing **No** from a browser
reached over your tailnet closes that tab's own connection, which the row says before
you pick it. See [Changing the mode while dux is serving](/docs/tailscale#changing-the-mode-while-dux-is-serving).

Each row shows its documented default and, where `0` means something special like "never
auto-clear", that meaning too. Values are validated and clamped on save, and every
connected browser refreshes once it is written.

The seven timing settings that decide how a browser rides out a bad network
(`replay_wait_seconds`, `reconnect_backoff_cap_seconds`, `reconnect_attempts`,
`reconnect_attempt_timeout_seconds`, `changes_request_timeout_seconds`,
`heartbeat_seconds` and `heartbeat_deadline_seconds`) are not in this panel either. They live under `[server]` in
the config file, documented inline with everything else, and a config reload applies them
to every open tab. See
[the `[server]` config keys](/docs/server-mode#the-server-config-keys) for what each one
does and its default.

Settings that only affect the terminal UI, such as its theme or the diff viewer's tab
width and line numbers, are not in this panel. Keybindings, provider commands, and project
identity stay in the raw config file or their own dialogs. Use the cog menu's
**Configuration → Edit config file…** for anything not covered.
