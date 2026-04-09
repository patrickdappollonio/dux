# CLAUDE.md

## Design Tenets

Principles that guide every decision in dux. If a change conflicts with a tenet, the tenet wins.

### Configuration

- **All settings are configurable.** Every single one. If a user can't change it, it shouldn't be hardcoded.
- **The config file is the documentation.** It must clearly explain what each setting does through inline comments. A user should never need to leave the config file to understand an option.

### UI and Navigation

- **Tab and Shift-Tab navigate between panes.** This is the primary spatial navigation model.
- **Panes have local key combinations.** A key combo bound in one pane does not necessarily work in another.
- **Panes have interactive and non-interactive modes.** In interactive mode, all key combos (including global ones) are suppressed and input is forwarded to the PTY. In non-interactive mode, both local and global key combos are active.
- **Key combinations are documented in the help page.** The in-app `?` overlay is the authoritative reference. External docs describe how to configure bindings, not enumerate them.
- **Line-scroll keys are gated by context.** In interactive (PTY) mode, arrow keys and Space only scroll when already scrolled back (`scrollback_offset > 0`); otherwise they pass through to the child process. In non-interactive views (diff overlay, non-interactive agent pane, help overlay), there is no competing use for these keys, so they scroll unconditionally. Page-scroll keys (`PgUp`/`PgDn`) always initiate scrolling regardless of context.
- **Animations and periodic refreshes use wall-clock time, not tick counts.** Tying visual updates to tick rate couples frame cadence to logic timing. Use `Instant::elapsed()` (or equivalent) so animations stay smooth and refreshes stay consistent regardless of how often ticks fire.

### Agents and Providers

- **A provider is supported if and only if it supports PTY and oneshot mode.** PTY for interactive sessions; oneshot (headless: send a prompt, get one response) for automated tasks like commit message generation.
- **Any CLI tool can be a provider.** Configure `command` in `config.toml` and dux spawns it. No adapters, no protocol layer. Adding a new provider is a config-only change, not a code change.
- **Claude, Codex, OpenCode, and Gemini CLI are the defaults.**
- **No protocol layer.** No JSON-RPC, no custom message format, no adapter binaries. The CLI runs exactly as it would in a normal terminal.

### Git and Data Safety

- **Worktrees are user data.** Never removed or mutated casually. Deletion requires explicit user confirmation.
- **Git operations are conservative.** Source checkout refresh uses `--ff-only`. Destructive operations require confirmation.
- **Prefer explicit failure over silent waiting.** If something fails, say so immediately with context.

### Tone

- **Welcome tips are playful and sassy.** They should feel fun, not like a manual. Lead with surprise or delight, highlight non-obvious features that differentiate dux, and keep keybinding references secondary to the feature discovery. Avoid dry "press X to do Y" phrasing.

### Quality

- **Prove your work with tests.** Every change should include unit tests. When feasible and low-lift, add integration tests as well.
- When debugging a problem, before fixing it, **aim to prove your findings with a unit test**.

## Project Summary

`dux` is a Rust terminal UI for orchestrating AI coding sessions per git worktree.

The current app provides:

- A left pane for projects and agent sessions
- A center pane showing the agent's terminal output (embedded via PTY) or diff viewing
- A right pane for changed files and diffs
- Commented user config in the platform-specific dux config directory (`~/.dux/` on macOS, `~/.config/dux/` on Linux)
- Session persistence in `sessions.sqlite3` alongside the config
- Logging in `dux.log` alongside the config
- PTY-based agent startup: spawns CLI tools (`claude`, `codex`, `opencode`, `gemini`) directly in a pseudo-terminal

## Important Constraints

- The app spawns CLI tools directly via PTY (portable-pty) and renders their output using an embedded terminal emulator built on alacritty_terminal. There is no protocol layer (no ACP, no JSON-RPC).
- Long-running actions should not block the UI thread.
- All periodic or potentially-blocking work (git commands, file I/O, network) must run in background workers, never on the main UI thread. Even fast operations like `git symbolic-ref` should use workers to prevent UI freezes if the filesystem or git lock stalls.
- User state lives under `~/.dux/` on macOS and `$XDG_CONFIG_HOME/dux/` (or `~/.config/dux/`) on Linux.
- Worktrees are user data and should not be removed or mutated casually.
- Git operations should be explicit and conservative, especially around the source checkout.
- **Target platforms are macOS and Linux only.** Windows users run dux through WSL2, which is Linux. Do not add `#[cfg(windows)]` branches, `cfg!(windows)` checks, or Windows-specific code paths. Assume Unix throughout.

## App Module Structure

The `src/app/` directory splits the TUI into focused submodules. Each file contains an `impl App` block for a specific concern:

- **`mod.rs`** — `App` struct, types, enums, bootstrap, run loop, and state helpers. Read this first to understand the data model.
- **`input.rs`** — All keyboard/mouse event handling (`handle_key`, `handle_left_key`, etc.).
- **`render.rs`** — All rendering methods (`render`, `render_header`, `render_left`, etc.) and UI helper functions.
- **`sessions.rs`** — Project and session CRUD (create agent, delete, reconnect, refresh, project browser).
- **`workers.rs`** — Background thread management (`drain_events`, `spawn_*`, `run_create_agent_job`).

When making changes, edit only the relevant submodule. If you need to add a new method to `App`, place it in the submodule that matches its concern. If adding a new type or enum, add it to `mod.rs`.

## Recommendations For Future Changes

- Keep the status line centralized. New async operations should report progress through the shared status API, not ad hoc strings.
- Status line messages must be verbose and actionable. Do not write terse messages like "Done." or "Pushed." — instead explain what happened and, when relevant, what the user can do next. Example: "Changes committed successfully. Press ^U to push to remote." rather than "Committed."
- Code must be production ready and modular to allow future Open Source contributors to integrate new features with ease. Do not take shortcuts.
- Route new modal UI through `PromptState` so `Esc` keeps working uniformly.
- Prefer command-palette actions over adding many more global hotkeys.
- Keybinding labels shown to the user should use proper casing (e.g. `Ctrl-g`, not `ctrl-g` or `CTRL-G`). Title-case modifiers (`Ctrl`, `Shift`, `Alt`) distinguish them visually from the key itself. The `^X` notation (e.g. `^P`) is acceptable in footer hint bars since its meaning is explained in the help overlay.
- Keybinding parsing from config is case-insensitive for modifiers and control keys — `CTRL-g`, `Ctrl-g`, and `ctrl-g` all parse identically (crokey lowercases the input). Letter keys are also lowercased during parsing, so `P` and `p` are the same binding; to bind uppercase P the user must write `shift-p`.
- Never hardcode keybinding labels in user-facing strings (config comments, status messages, UI text). All keybindings are user-configurable — always look up the actual binding via `RuntimeBindings::label_for()` so labels stay accurate after rebinding. The only exception is pure text-input contexts (typing characters, backspace, cursor movement in text fields).
- Keep agent creation and other blocking git/provider work in background workers.
- The PTY approach is CLI-agnostic: any terminal command can be used as a provider. Keep it generic.

## Keeping Documentation In Sync

- When adding, removing, or changing keybindings, update `README.md` if it documents controls or shortcuts.
- When changing features, pane layouts, config paths, or provider behavior, verify that `README.md` still accurately describes the application.
- The README intentionally omits a full keybinding reference to avoid staleness. Do not add a section listing specific key combinations back to it. The in-app `?` help overlay is the authoritative reference for key combinations. Documenting how to configure keybindings (the `[keys]` config format, syntax, and examples) is acceptable — just avoid enumerating individual bindings that would go stale.

## Recommendations For Debugging

- First check the `dux.log` file in the dux config directory (`~/.dux/` on macOS, `~/.config/dux/` on Linux).
- Confirm whether the provider command in `config.toml` is installed and on PATH.
- If agent creation fails, determine whether it stopped in:
  - worktree creation
  - PTY spawn (command not found)
  - PTY process early exit
- If the UI appears frozen, verify the operation is on a worker path rather than the main event loop.

## Git Command Safety

When shelling out to git, **always ensure the command output is immune to user-specific git configuration**. User settings like `color.diff`, `diff.noprefix`, `status.branch`, `status.renames`, `core.quotePath`, and others can alter output in ways that break parsing.

- **Prefer plumbing commands** (`cat-file`, `rev-parse`, `for-each-ref`) over porcelain when you need machine-readable output. Plumbing commands are guaranteed stable.
- **Use `--porcelain`** for `git status`. Never use `--short` — it is affected by user config (`status.branch`, `status.relativePaths`, etc.).
- **Prefer `--porcelain=v1 -z` when parsing status paths.** Parse NUL-delimited records instead of line-based output so spaces, quotes, Unicode, and embedded newlines in paths are handled safely.
- **Use `--numstat`** for line-level diff statistics. It outputs tab-separated values unaffected by config.
- **Prefer `--numstat -z` when parsing file paths from diff stats.** Rename and copy records should be parsed from NUL-delimited output, not from whitespace- or newline-delimited text.
- **Override config with `-c`** when a porcelain/plumbing alternative doesn't exist (e.g., `git -c color.diff=false diff`).
- **Imperative commands** that aren't parsed (`worktree add`, `branch -D`, `pull`) are fine as-is.
- **For imperative git commands, rely on exit status rather than parsing stdout.** Treat stdout/stderr as user-facing diagnostics only unless Git explicitly documents the format as machine-stable.
- **Do not parse human-facing commands like `git branch --show-current` when a plumbing-style alternative exists.** Prefer commands such as `symbolic-ref --quiet --short HEAD` or `rev-parse`.
- **Avoid shelling out to `git diff` for display** — the project computes diffs in-process using the `similar` crate and applies syntax highlighting with `syntect`. Use `git::file_at_head()` to get the base version and read the working copy from disk.

## Recommendations For Editing

- Follow the existing `src/app/` submodule pattern when adding new concerns. If a new feature area grows beyond ~200 lines, extract it into its own submodule with `use super::*;` and an `impl App` block.
- Keep changes scoped to one submodule at a time; avoid cross-cutting edits across multiple app submodules in the same PR when possible.
- Use `theme.rs` constants for all colors and styles — never use raw `Color::*` values in rendering code.
- When adding new UI elements, define semantic color names in `Theme` rather than picking ad-hoc colors. `theme.rs` is the single source of truth for visual styling.
- The canonical config renderer produces a fully commented config on first creation. Subsequent saves preserve user edits via `toml_edit`. Users can run `dux config diff` to see what changed or `dux config regenerate` to get the latest canonical template.
- When a setting can have a sensible default at first boot (e.g., the user's home directory, platform-specific paths), resolve and store the concrete value in `config.toml` right away — do not leave it commented out or empty. Users should see a working value they can edit, not a placeholder they have to fill in.
- Preserve safe failure behavior around project refresh and failed agent startup.

## Verification

Use:

```bash
cargo fmt
cargo test
cargo run
```
