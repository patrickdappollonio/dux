# dux

This file and each `AGENTS.md` in an area directory hold what an agent must know before changing dux: rules stated as facts, at most 200 lines per file, with no history (that is in git).
When a change conflicts with a rule here, the rule wins.

## What dux is

- dux is a Rust terminal UI, with a web UI in server mode, that runs AI coding CLIs in pseudo-terminals, one git worktree per agent.
- `crates/dux-core` is the engine and the model both surfaces render; `crates/dux-tui` is the terminal UI; `crates/dux-web` is the server, and `crates/dux-web/web/` its browser UI (a path written `lib/…` below is under `crates/dux-web/web/src/`); `crates/dux` is the command line; `website/` is the site and docs.
- User state lives in `~/.dux/` on macOS and in `$XDG_CONFIG_HOME/dux/` (else `~/.config/dux/`) on Linux: `config.toml`, `sessions.sqlite3`, `dux.log`.
- Targets are macOS and Linux only; Windows users run dux in WSL2, so the code assumes Unix throughout.
- A provider is any CLI named by `command` in `config.toml` and run in a PTY: no adapters, no protocol layer, no JSON-RPC. The defaults are `claude`, `codex`, `opencode`, `copilot` and `grok`.

## Configuration

- Every setting is configurable. The exceptions are the starter `.gitignore` list in `crates/dux-core/src/gitignore_seed.rs`, because it decides what dux writes into new repositories, and `COMMAND_ATTACHMENT_CHAR_LIMITS` in `lib/fileDrop.ts`, because it records a third-party CLI's limit; both change by pull request.
- The canonical config explains every setting in an inline comment, so a user never needs another document to understand one.
- When a setting has a sensible default at first boot, resolve it and write the concrete value into `config.toml`.
- `[[projects]]` holds portable intent only: id, path, name, default provider, auto-reopen, startup command.
- Derived state (branch status, worktree paths, agent branch names, the leading branch, provider process and session state, tabs) lives in SQLite or memory, never in config.
- A project's startup command text is project config; the shell that runs it is the global `[startup_command_terminal]` section.
- For a project in both config and SQLite, explicit config values for preference fields overwrite SQLite on startup and reload, SQLite fills only the fields config lacks, and only duplicate ids or disagreeing paths are sync conflicts.

## User data and git

- Worktrees are user data: never remove or change one without the user's explicit confirmation.
- A standalone agent's folder that the user chose is never created, moved or removed by dux, and a request to remove it is refused. A folder dux created for a standalone agent is removed with the agent unless the delete says to keep it, and only while the folder still holds dux's mark.
- dux deletes only branches it created, unless the delete dialog's branch checkbox answers otherwise; with no dialog (project removal, create rollback, factory reset) the branch's recorded provenance decides.
- Every destructive action asks first, in a dialog with Cancel focused and a destructive-styled confirm; a dialog keyed to a target closes when the target disappears.
- Git operations are explicit and conservative; refreshing a source checkout uses `--ff-only`.
- Every git call that takes a refname puts `--` before it; `git worktree add` takes a resolved object id, and `git pull` and revision slots such as `rev-list` take `refs/heads/<name>`. Each case gets a test in the option-looking-name tests in `crates/dux-core/src/git.rs`.
- Git output must not depend on the user's git config: prefer plumbing (`rev-parse`, `cat-file`, `for-each-ref`, `symbolic-ref --quiet --short HEAD`), and parse `git status --porcelain=v1 -z` (never `--short`) and `--numstat -z`.
- Where no stable form exists, override config with `-c` (`git -c color.diff=false ...`); imperative commands (`worktree add`, `branch -D`, `pull`) are judged by exit status, never by their output.
- Diffs for display are computed in-process with `similar` and `syntect`, from `git::file_bytes_at_head()` and the working copy; never shell out to `git diff` for display.
- Files dux keeps for itself are owner-only, decided in `crates/dux-core/src/file_modes.rs`; dux never tightens the mode of user data (worktrees, the lock, user theme directories).
- Commit messages are plain sentences: no conventional-commit prefixes, no trailers.

## Work and status

- Git, file I/O, network and anything periodic or possibly blocking run in background workers, never on the UI thread or an event loop; `git symbolic-ref` included.
- Fail at once and say why with context; never wait silently.
- `dux_core::statusline` is the one source of status for both surfaces: every async operation sets a keyed Busy when it starts and a final success or failure under the same key when it ends.
- A path that emits a keyed Busy registers that key as live with the controller; never infer liveness by enumerating operation registries. A multi-step web operation never emits an unkeyed Busy.
- Status messages are full sentences that say what happened and what the user can do next; never "Done." or "Pushed.".
- A success is confirmed unless the screen shows it unmistakably. A destructive act, a change to a small indicator, an action that found the state already held, and an effect that can look like a no-op are always confirmed, on both surfaces.
- The surfaces for status are the TUI's status line and the web's toasts; the command line prints operation records and is not a status surface.

## Across both surfaces

- Escape never saves: it abandons, or leaves edit mode.
- Animations and periodic refreshes run on elapsed wall-clock time, never on tick counts.
- Adjacent click targets are spaced so an imprecise click cannot hit the wrong one.
- A name inside a sentence (branch, path, file, command, agent, project, terminal, provider, pull request) is a chip, never quoted: `name_chip` in the TUI, `InlineCode` on the web. Build the sentence from prose segments; never style a name by hand or find one by parsing a sentence.
- A sentence both surfaces print is built once as `Prose` in `dux-core`, and `lib/prose.tsx` builds the same segments for the browser.
- Key bindings are configurable: never write a key's name into code, copy or docs; resolve its label through the bindings (`RuntimeBindings::label_for()` in the TUI).
- The in-app `?` overlay is the reference for key bindings; `README.md` and docs explain how to configure bindings and never list them.
- A new action goes into the TUI's command palette (`dux_core::palette`), never a new global hotkey; the web's equivalent is the app menu (`lib/appMenu.ts`), which has no keyboard shortcut.
- A pull request that adds a palette command states whether the web menu and the command line get it too.
- Never conflate the TUI palette and the web app menu in docs or copy; scope a sentence that names a key to its surface; never suggest `ctrl-k` as a TUI rebind.
- A click on an OSC 8 link in a terminal opens the link in dux and never reaches the program, even under mouse tracking; the hatch chord (on the web Cmd on macOS and Ctrl elsewhere, in the terminal UI Ctrl) forwards the click instead.
- File editing is web-only; the TUI has no editor.
- Welcome tips are playful and sassy: lead with a surprising feature, keep key references secondary.

## Agents, tabs and terminals

- An agent's workspace is a managed working copy or a folder (standalone) that the user chose or dux created; every question about it is a named method with an exhaustive match, never a `matches!` at a call site.
- Every git action resolves the workspace through the engine's chokepoint, which refuses with a sentence; an unclassified folder refuses every write.
- A standalone agent has no branch features (push, pull, fork, pull requests, worktree manager, startup command, project environment): they are absent, not disabled.
- Every background enumerator (branch sync, pull-request sync, the changes watcher) checks the workspace before enrolling an agent; nothing polls a plain folder.
- A tab is a provider session inside its agent's one worktree, never a list row of its own; every tab, the first included, has a stable id for its whole life.
- Whether a tab is the first is answered only by the session's pointer through the shared resolvers; never compare a tab id with a session id.
- Every launch and reopen path decides resume through `Engine::tab_resume_decision`.
- A terminal's owner is an agent session, a project, or nothing (standalone); every ownership question is an exhaustive method on `TerminalOwner`.
- Terminals are runtime-only: no SQLite row, no config entry, no restore after a restart.
- A terminal row and an agent row render through the same shared row component on each surface.
- `dux_core::flat_list::order_rows` and `orderFlatRows` in `lib/flatList.ts` order the agents list; change both, and the shared fixture `crates/dux-core/tests/fixtures/flat_list_cross_language.json`, together.

## Driving a row

- A row is an agent with its tabs and terminals, or one project or standalone terminal; one device drives it at a time, and only a press of Take over moves it to another.
- While a device drives a row, every other device and client is refused anything that changes that agent or writes in its folder.
- `Engine::admit_row_change` in `crates/dux-core/src/engine/row_guard.rs` is the one question; a new request that changes a row gets a `RowChange` variant, turned into rows in `Engine::rows_changed`, never a check at its call site.
- A change that would end a terminal another connection has open is refused, naming who, through the attachment registry in `dux-core`; `force_connected=true` goes ahead over it.

## Command line

- The command line reads and changes SQLite-backed state only through a running dux (the control socket, or a remote's HTTP listener); with no dux running it says so and stops.
- Resources kept in `config.toml` (macros, providers, keys, themes, env) and `server.log` are files; the command line edits them directly while dux is stopped.
- The target is `--local`, else `--remote`, else `DUX_REMOTE`, else the saved default remote, else this machine; a target that does not answer stops the command and nothing falls back.
- `dux config` edits this machine's file and refuses while a remote is selected unless `--local` is given; a destructive prompt names its target.

## Code

- Never truncate a user-visible string by bytes (`.len()`, `[..n]`); count and cut by chars.
- Prove a change with tests, sized to its risk. A test must be able to fail: none that only restates the code. When debugging, write the failing test before the fix.
- When a change affects anything `README.md` or `website/` describes (features, pane layouts, config paths, provider behavior), update them in the same change.

## Verification

- Run `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test`; clippy is a CI gate, and a new lint is fixed in the code, not suppressed.
- The web UI is compiled into the binary and a failed frontend build fails `cargo build`; `DUX_DISABLE_UI_BUILD` skips that build for Rust-only work.
- See the web UI only through the preview container in `tools/preview-env/` (its `README.md` says how).
- Never run `dux`, `dux server` or `cargo run` on the host, and never kill a dux process there: they share the developer's live config and session. Ask for a `cargo run` smoke test instead.
- Debugging starts at `dux.log` in the config directory. A failed agent creation stopped in worktree creation, PTY spawn (the provider's command is not on PATH) or an early exit of the process.
