# AGENTS.md

## Project Summary

`dux` is a Rust terminal UI for orchestrating AI coding sessions per git worktree.

The current app provides:

- A left pane for projects and agent sessions
- A center pane for live agent output or diff viewing
- A right pane for changed files and diffs
- Commented user config in the platform-specific dux config directory (`~/.dux/` on macOS, `~/.config/dux/` on Linux)
- Session persistence in `sessions.sqlite3` alongside the config
- Logging in `dux.log` alongside the config
- ACP-oriented agent startup with timeout handling

## Important Constraints

- The app assumes providers are ACP-compatible adapters, not raw `codex` or `claude` CLIs.
- Long-running actions should not block the UI thread.
- User state lives under `~/.dux/` on macOS and `$XDG_CONFIG_HOME/dux/` (or `~/.config/dux/`) on Linux.
- Worktrees are user data and should not be removed or mutated casually.
- Git operations should be explicit and conservative, especially around the source checkout.

## Key Files

- [src/app.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/app.rs): main TUI, event loop, commands, overlays, async agent creation
- [src/acp.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/acp.rs): ACP stdio JSON-RPC client
- [src/git.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/git.rs): git/worktree helpers
- [src/config.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/config.rs): config schema, defaults, rendering
- [src/storage.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/storage.rs): SQLite session persistence
- [src/statusline.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/statusline.rs): status line model and spinner behavior
- [src/theme.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/theme.rs): centralized color palette and semantic styling constants
- [src/logger.rs](/home/patrick/Golang/src/github.com/patrickdappollonio/dux/src/logger.rs): runtime logging

## Recommendations For Future Changes

- Keep the status line centralized. New async operations should report progress through the shared status API, not ad hoc strings.
- Route new modal UI through `PromptState` so `Esc` keeps working uniformly.
- Prefer command-palette actions over adding many more global hotkeys.
- Keep agent creation and other blocking git/provider work in background workers.
- Do not assume a provider speaks ACP just because its executable exists. Detect and fail fast.
- If ACP support grows, separate provider capability discovery from session creation.
- If session restore becomes richer, persist more structured runtime metadata instead of inferring from worktree presence alone.

## Keeping Documentation In Sync

- When adding, removing, or changing keybindings, update `README.md` if it documents controls or shortcuts.
- When changing features, pane layouts, config paths, or provider behavior, verify that `README.md` still accurately describes the application.
- The README intentionally omits a full keybinding reference to avoid staleness. Do not add a Controls or Keybindings section back to it. The in-app `?` help overlay is the authoritative reference for key combinations.

## Recommendations For Debugging

- First check the `dux.log` file in the dux config directory (`~/.dux/` on macOS, `~/.config/dux/` on Linux).
- Confirm whether the provider command in `config.toml` is a real ACP adapter.
- If agent creation fails, determine whether it stopped in:
  - worktree creation
  - provider process spawn
  - ACP `initialize`
  - ACP `session/new`
- If the UI appears frozen, verify the operation is on a worker path rather than the main event loop.

## Recommendations For Editing

- Keep changes small and composable in `src/app.rs`; it is large enough that unrelated edits can conflict.
- Prefer extracting helper types/modules when adding new UI modes or async workflows.
- Use `theme.rs` constants for all colors and styles — never use raw `Color::*` values in rendering code.
- When adding new UI elements, define semantic color names in `Theme` rather than picking ad-hoc colors. `theme.rs` is the single source of truth for visual styling.
- Preserve the fully materialized commented config behavior.
- When a setting can have a sensible default at first boot (e.g., the user's home directory, platform-specific paths), resolve and store the concrete value in `config.toml` right away — do not leave it commented out or empty. Users should see a working value they can edit, not a placeholder they have to fill in.
- Preserve safe failure behavior around project refresh and failed agent startup.

## Verification

Use:

```bash
cargo fmt
cargo test
cargo run --bin dux
```
