# CLAUDE.md

## Design Tenets

Principles that guide every decision in dux. If a change conflicts with a tenet, the tenet wins.

### Configuration

- **All settings are configurable.** Every single one. If a user can't change it, it shouldn't be hardcoded.
- **The config file is the documentation.** It must clearly explain what each setting does through inline comments. A user should never need to leave the config file to understand an option.
- **Project config is portable desired state, not runtime cache.** `[[projects]]` may store portable user intent such as `id`, env-expanded `path`, `name`, `default_provider`, `auto_reopen_agents`, and `startup_command`. Do not write runtime-derived git or agent state into config.
- **Keep derived project state in SQLite.** Values such as `leading_branch`, `current_branch`, branch status, agent worktree paths, agent branch names, and provider process/session state belong in SQLite/runtime memory only. In particular, `leading_branch` may be parsed from older config files to repair SQLite, but saved/generated config must omit it so autodetection is not pinned into portable config.
- **Startup command execution shell is global config, not project state.** Project config owns the command text; `[startup_command_terminal]` owns the system-wide shell and args used to run it. Keep this out of SQLite and UI-only state so shell behavior stays portable and reviewable in config.
- **Config wins for explicit project preferences.** When a project exists in both config and SQLite, explicit config values for safe preference fields should update SQLite on startup/reload. SQLite may fill missing config fields, but should not override a value the user wrote in config. Reserve hard sync conflicts for project identity ambiguity: duplicate ids/paths, same id with different expanded paths, or same expanded path with different ids.

### UI and Navigation

- **New UI must use the theme engine.** Any new screen, pane, dialog, status text, or visual state must derive colors and styles from `Theme`/`theme.rs` rather than hardcoding visual values. Prefer reusing an existing semantic theme field when it already matches the visual meaning. Only add a new semantic theme field when the existing theme surface truly lacks the needed meaning, and when you do, wire every supported theme/default mapping through it in the same change so themes cannot drift.
- **Tab and Shift-Tab navigate between panes.** This is the primary spatial navigation model.
- **Panes have local key combinations.** A key combo bound in one pane does not necessarily work in another.
- **Panes have interactive and non-interactive modes.** In interactive mode, all key combos (including global ones) are suppressed and input is forwarded to the PTY. In non-interactive mode, both local and global key combos are active.
- **Key combinations are documented in the help page.** The in-app `?` overlay is the authoritative reference. External docs describe how to configure bindings, not enumerate them.
- **Line-scroll keys are gated by context.** In interactive (PTY) mode, arrow keys and Space only scroll when already scrolled back (`scrollback_offset > 0`); otherwise they pass through to the child process. In non-interactive views (diff overlay, non-interactive agent pane, help overlay), there is no competing use for these keys, so they scroll unconditionally. Page-scroll keys (`PgUp`/`PgDn`) always initiate scrolling regardless of context.
- **Space activates the focused button in confirmation dialogs.** Every modal that presents buttons (confirm/cancel, delete/cancel, etc.) must treat Space as equivalent to Enter — it executes whichever button is currently highlighted. This is hardcoded, not a configurable keybinding, because it is a universal accessibility convention. New dialogs with buttons must include this behavior.
- **Clickable controls need misclick-safe spacing.** Separate adjacent click targets enough that an imprecise click cannot accidentally activate a different item. When a checkbox sits above modal buttons, include a blank row or equivalent spacing between the checkbox and the buttons.
- **Animations and periodic refreshes use wall-clock time, not tick counts.** Tying visual updates to tick rate couples frame cadence to logic timing. Use `Instant::elapsed()` (or equivalent) so animations stay smooth and refreshes stay consistent regardless of how often ticks fire.
- **The embedded-PTY caret is the host hardware cursor, never a painted block.** Render the agent/terminal cursor by moving the real terminal cursor onto the PTY cursor cell (`set_cursor_position`) and never pre-paint that cell into a styled cursor block. We do this deliberately for **accessibility and UX with CJK IME**: the OS anchors composition popups (Korean/Japanese/Chinese input) to the hardware cursor, so without it composition renders at the terminal origin instead of the agent prompt. Pre-painting the cell as well also reintroduces an invisible-caret bug under hosts like Alacritty that draw their block cursor by inverting the cell (inverting an already-styled cursor cell cancels it out). This means the caret look is the host terminal's cursor and is not theme-colored; that tradeoff is intentional.
- **The TUI does not disambiguate Shift-Enter; Ctrl-J is the supported soft-newline key.** In interactive mode the TUI forwards raw stdin bytes to the child PTY verbatim and never pushes the Kitty keyboard-enhancement flags (`PushKeyboardEnhancementFlags`). Under the legacy terminal protocol the host sends the *same* byte (CR, `\r`/0x0D) for both Enter and Shift-Enter, so there is no distinct Shift-Enter for dux to forward. Making it distinct would require pushing enhancement flags, which (a) only works on host terminals that implement the Kitty protocol and (b) changes the encoding of *every* key — and because interactive input is a verbatim passthrough, those re-encoded bytes reach the child, so it would break providers that don't speak the protocol. We deliberately do **not** do this. `Ctrl-J` already sends LF (0x0A), which agent CLIs treat as a soft newline, so it is the supported way to insert a newline without submitting in the TUI. Users who want a literal Shift-Enter binding can get it by running dux inside a multiplexer/terminal already configured for the Kitty protocol (e.g. tmux). Only the **web UI** rewrites Shift-Enter to LF (see below), because there the browser key event is available before xterm encodes it.

### Web UI (server mode)

The web UI (`crates/dux-web/web/`, React + Vite + Tailwind v4) has its own conventions, distinct from the TUI's `theme.rs` engine above. It is currently **dark-only** (`main.tsx` force-adds the `.dark` class); the light `:root` tokens exist but are inert. Style through the shadcn/base-ui token CSS variables (`--primary`, `--muted-foreground`, `--destructive`, …), never hardcoded colors.

- **Components: shadcn/base-ui first.** Reuse an existing `components/ui/*` primitive (`DropdownMenu`, `Dialog`, `Button`, `Tooltip`, …) before hand-rolling one. Hover hints go through the shared `SimpleTooltip`, never a native `title=`.
- **Row actions collapse into one `⋯` menu.** When a list row (changed files, projects, sessions) needs more than one action, put them in a single `DropdownMenu` opened by an `Ellipsis`/`EllipsisVertical` trigger — not a strip of inline buttons. The trigger reveals on hover and consumes **no layout space** when idle: animate it in with a `max-width`+`opacity` transition (or, in the sidebar, the absolute `translate-x`+`opacity` of `SidebarMenuAction showOnHover`). Keep it revealed while the menu is open via the Base UI trigger's `data-popup-open` attribute (`md:has-[[data-popup-open]]:…`, or `data-[popup-open]:…` on the trigger itself) — Base UI does **not** set `aria-expanded=true` on an open menu trigger, so keying off `aria-expanded` silently never matches and the action un-reveals (dragging the anchored popover) the instant the cursor leaves the row. Always show it on touch (`max-md:` overrides), and add `motion-reduce:transition-none`. The trigger must `stopPropagation` so it doesn't fire the row's own click.
- **Menu items: icon + neutral color; "…" marks a further step.** Every item in a dropdown `⋯` menu keeps a leading lucide icon — this applies to all of them (agent, Changes, project); none ship an iconless item. (The Ctrl-K command palette is intentionally *not* an icon menu — it is a two-column text table, so this rule does not extend to it.) Append a trailing ellipsis `…` to any item that opens a dialog or needs confirmation (matching the Ctrl-K palette and the project/session menus). **Default destructive items to neutral color** — the `…` plus the confirmation dialog are the danger signal, not the color. (Most destructive `DropdownMenuItem`s use neutral text; reserve `text-destructive`/`variant="destructive"` for confirm buttons inside the dialog.) **The one deliberate exception is the agent ⋯ menu's "Delete agent…" entry**, which is tinted red at rest and brighter on hover (`variant="destructive"` plus `not-focus:text-destructive/70!` to dim the rest state). It is the most consequential per-agent action, so it carries the extra visual weight; the confirmation dialog still gates it. Do not propagate this red treatment to other destructive menu items without a deliberate decision.
- **Destructive actions always confirm.** A destructive item opens a dedicated `Dialog` (Cancel `autoFocus`, the confirm button `variant="destructive"`, misclick-safe spacing) gated by a store target — follow the existing per-action dialog pattern (e.g. `ConfirmDiscardFileDialog`); there is no shared `useConfirm` hook.
- **Shift-Enter inserts a soft newline (LF), not a submit.** The terminal pane intercepts keystrokes at the key-event layer via xterm's `attachCustomKeyEventHandler`, because xterm collapses both Enter and Shift-Enter to CR before `onData` sees them — they are indistinguishable at the data layer. The decision lives in the pure, unit-tested helpers in `lib/termkeys.ts`, not in the component: `softNewline(event)` matches only a bare Shift-Enter `keydown` (no other modifier, and **never mid-IME-composition** — it bails on `isComposing`/`keyCode === 229` so a CJK confirm keystroke is never rewritten), and `softNewlineAction(event, ctx)` folds in the runtime context (ownership, armed latch) to return what the handler must do. The component closure is a thin applicator: on a match it `preventDefault`s (so the browser drops no stray newline into the hidden textarea) and `stopPropagation`s (so the handled key doesn't bubble to window shortcuts) and returns `false` (so xterm emits no CR); only the input owner then writes the `LF` (0x0A, the Ctrl-J byte), and only the owner's path scrolls to the live edge, clears any selection, and consumes an armed one-shot Ctrl/Alt latch — matching the side effects `onData` would otherwise provide. Every other key returns `true` and is encoded by xterm as usual. Because a mobile soft keyboard can't produce the chord, the mobile accessory bar carries a dedicated `⇧↵` key as a second entry point; both it and the physical-key handler write through the one shared `writeSoftNewline(term, pty)` helper so their landing behavior (scroll/clear/send) can never drift. This is web-only on purpose (the TUI cannot disambiguate Shift-Enter — see the TUI tenet above). Keep the branching in the `termkeys.ts` helpers so it stays testable without mounting xterm.
- **One marker for git status.** Render every changed-file status marker through the shared `FileStatusIcon` (lucide `File-*` icons), in the changes pane and the editor's tree/search alike — don't re-implement it inline. The raw status is interpreted once by the pure, unit-tested `fileStatusMeta(status) → { kind, label }` in `lib/changedFiles.ts`; the icon is neutral with a tooltip label.
- **Touch targets ≥44px.** Interactive controls get a `max-md:min-h-11` / `max-md:size-11` (44px) target on phones; desktop density is restored with `md:`.
- **The working-agent icon bob is required, and must settle back to rest.** While an agent streams output (`session.working`, the server-sourced boolean), its `Bot` icon bobs in the sidebar and mobile rows (`--animate-agent-working` / `@keyframes agent-working` in `index.css`) as a deliberate "working" cue. Do not remove it as "cleanup." Two hard requirements: (1) it bounces only while working; (2) when work stops it must glide back to the rest position (`translateY(0)`) and never freeze at the top or bottom of the bounce (or anywhere mid-cycle). Requirement 2 is why the icon always carries `motion-safe:transition-transform motion-safe:duration-300` next to the conditional `motion-safe:animate-agent-working`: dropping the animation class returns the transform to its base value and the transition eases it back to rest. Keep both classes together, and keep the keyframes resting at `0%`/`100%`. The bob and the name shimmer (`.agent-name-shimmer`) are the row's two working cues; both fire on the same active-and-working flag from `agentRowVisual` so they stay in lockstep.

### Agents and Providers

- **A provider is supported if and only if it supports PTY mode.** dux embeds the CLI's interactive session in a pseudo-terminal; that is the only thing a provider has to support.
- **Any CLI tool can be a provider.** Configure `command` in `config.toml` and dux spawns it. No adapters, no protocol layer. Adding a new provider is a config-only change, not a code change.
- **Claude, Codex, OpenCode, and Copilot are the defaults.**
- **No protocol layer.** No JSON-RPC, no custom message format, no adapter binaries. The CLI runs exactly as it would in a normal terminal.

### Agent Tabs

- **A tab is a provider session inside the agent's one shared worktree, not a new worktree.** One `AgentSession` (one branch, one sidebar row) hosts N tabs; every tab runs against the same checkout and edits the same files. The unified tab strip also carries the agent's companion terminals as terminal tabs. Tabs are never separate sidebar agents.
- **Exactly one tab resumes, and it is positional (leftmost).** Only the leftmost provider tab launches with the provider's resume flag (e.g. `--continue`); every other provider tab always starts fresh, including after a restart-restore. This is a hard rule, not a nicety: provider `continue` is directory-scoped, not conversation-id-scoped, so two resuming tabs would reopen the *same* most-recent conversation and show identical content. Decide eligibility in one place (`resume_eligible_tab`) and make `should_resume_session` delegate to it, so the render badge, the spine, and every launch/reconnect path agree on which single tab resumes.
- **The primary tab is the agent, pinned leftmost.** Closing the primary tab detaches the agent (it stays in the sidebar, reopenable); closing a secondary tab removes it, which is destructive and not recoverable, so confirm it while its process is live (see the Destructive-actions tenet). Only secondary tabs reorder. Deleting the agent tears down *every* tab's PTY, not just the primary's.
- **New tabs default to the project provider and start fresh; provider is per-tab.** Adding a tab spawns the project `default_provider` immediately; retarget a tab via `ctrl+p` (TUI) or its `⋯` menu (web), effective on the tab's next launch (a pending reconnect), mirroring the existing agent-provider change.
- **Tab state is derived runtime state: keep it in SQLite, never in config.** Persist secondary tabs in an additive `agent_tabs` table (backward compatible — existing databases start with zero rows and behave exactly as before). The primary tab has no row; it is derived from the `agent_sessions` row. Do not write tab provider or order into portable project config (see the Configuration tenets on portable-vs-derived state).
- **Runtime provider state is keyed by tab id, with the primary tab id equal to the session id.** The `providers` map, `running_provider_pins`, resume-fallback state, and the PTY activity/streaming maps all key by tab id; keeping `tab_id == session_id` for the primary keeps every existing session-keyed lookup working. Session-scoped labels (sidebar, agent title) still resolve the primary tab; only the center pane resolves the focused tab.
- **Tabs honor each surface's existing UI tenets.** TUI: the strip is themed through `theme.rs` (`dux.tab_*` tokens wired through every theme), and tab actions are Center-scope, so they are inactive in fullscreen/interactive mode — exit fullscreen to switch. Web: Chrome-style pills below the commands bar, deep-linkable, reusing the shared `⋯`-menu, destructive-confirm, touch-target, and working-bob tenets above.

### Git and Data Safety

- **Worktrees are user data.** Never removed or mutated casually. Deletion requires explicit user confirmation.
- **Git operations are conservative.** Source checkout refresh uses `--ff-only`. Destructive operations require confirmation.
- **Commit messages are plain sentences.** Do not use conventional commit prefixes such as `feat:`, `fix:`, or `chore:`. Do not add structured commit trailers such as `Constraint:`, `Confidence:`, `Scope-risk:`, or `Tested:` unless the user explicitly asks for them.
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
- Async operations must keep the status line updated for the full operation lifecycle: set a Busy message when the worker starts, then replace it with a clear success or failure message when the worker completes. Do not start background work silently.
- User state lives under `~/.dux/` on macOS and `$XDG_CONFIG_HOME/dux/` (or `~/.config/dux/`) on Linux.
- **The web server (server mode) is single-tenant / trusted-access by design.** There is no login of any kind. The server binds `127.0.0.1` by default and optionally a Tailscale address; access control is delegated to loopback scope, Tailscale ACLs, or an upstream proxy the operator may add. A Host-header allowlist and same-origin check guard against DNS-rebinding and cross-site requests. Every client that reaches the server shares one workspace: it can attach to and drive any agent or terminal PTY, browse the server's filesystem (project picker), and see every session's status and changed files. This is intentional for a per-developer or trusted-team tool, with no per-user ownership or path sandboxing by design. Do not add features that assume mutually-distrusting web users without first designing a real per-user isolation model (session ownership enforced on subscribe/write, a sandboxed file browser, per-user status scoping).
- Worktrees are user data and should not be removed or mutated casually.
- Git operations should be explicit and conservative, especially around the source checkout.
- **Target platforms are macOS and Linux only.** Windows users run dux through WSL2, which is Linux. Do not add `#[cfg(windows)]` branches, `cfg!(windows)` checks, or Windows-specific code paths. Assume Unix throughout.

## App Module Structure

The `crates/dux-tui/src/app/` directory splits the TUI into focused submodules. Each file contains an `impl App` block for a specific concern:

- **`mod.rs`** — `App` struct, types, enums, bootstrap, run loop, and state helpers. Read this first to understand the data model.
- **`input.rs`** — All keyboard/mouse event handling (`handle_key`, `handle_left_key`, etc.).
- **`render.rs`** — All rendering methods (`render`, `render_header`, `render_left`, etc.) and UI helper functions.
- **`sessions.rs`** — Project and session CRUD (create agent, delete, reconnect, refresh, project browser).
- **`workers.rs`** — Background thread management (`drain_events`, `spawn_*`, `run_create_agent_job`).

When making changes, edit only the relevant submodule. If you need to add a new method to `App`, place it in the submodule that matches its concern. If adding a new type or enum, add it to `mod.rs`.

## Recommendations For Future Changes

- Keep the status line centralized. New async operations should report progress through the shared status API, not ad hoc strings.
- The core keyed status controller (`dux_core::statusline::KeyedStatusController`) is the single source of truth for engine status across the TUI and the web. The **TUI renders it as a single status line** (most-recent-wins; documented-lossy for concurrent statuses — it shows the most recent). The **web renders it as toasts** (sonner), one per open keyed status: success/info auto-clears after `ui.status_clear_seconds`, warning/error persist until a tied success or an explicit clear. **Correlation is by per-operation key:** a `Busy` and its eventual success/error/`StatusCleared` carry the same key so the surfaces replace/dismiss the right entry. Every `Busy` must be followed by a final (success, error, or clear); a keyed `Busy` left open expires to a Warning after the busy timeout. Do NOT add a third surface; do NOT emit an unkeyed `Busy` for a multi-step web operation (it would strand a toast at `Infinity`).
- Status messages auto-clear on a tone-aware policy owned by the controller: **Busy/pending persists until a final state replaces it** (a `set_busy` must be followed by a `set_info`/`set_error`; an unresolved pending intentionally shows as a stuck spinner rather than vanishing), **Info/success auto-clears** after `ui.status_clear_seconds` (default 6; 0 disables), and **Warning/Error persist until the next status replaces them**. New status call sites must follow the pending→final convention; never leave a Busy hanging. On the web surface, the key correlation ensures the right toast is dismissed when the final arrives.
- Status line messages must be verbose and actionable. Do not write terse messages like "Done." or "Pushed." — instead explain what happened and, when relevant, what the user can do next. Example: "Changes committed successfully. Press ^U to push to remote." rather than "Committed."
- Code must be production ready and modular to allow future Open Source contributors to integrate new features with ease. Do not take shortcuts.
- Route new modal UI through `PromptState` so `Esc` keeps working uniformly.
- Prefer command-palette actions over adding many more global hotkeys.
- Keybinding labels shown to the user should use proper casing (e.g. `Ctrl-g`, not `ctrl-g` or `CTRL-G`). Title-case modifiers (`Ctrl`, `Shift`, `Alt`) distinguish them visually from the key itself. The `^X` notation (e.g. `^P`) is acceptable in footer hint bars since its meaning is explained in the help overlay.
- Keybinding parsing from config is case-insensitive for modifiers and control keys — `CTRL-g`, `Ctrl-g`, and `ctrl-g` all parse identically (crokey lowercases the input). Letter keys are also lowercased during parsing, so `P` and `p` are the same binding; to bind uppercase P the user must write `shift-p`.
- Never hardcode keybinding labels in user-facing strings (config comments, status messages, UI text). All keybindings are user-configurable — always look up the actual binding via `RuntimeBindings::label_for()` so labels stay accurate after rebinding. The only exceptions are pure text-input contexts (typing characters, backspace, cursor movement in text fields) and palette-only command names (actions with no keybinding that are invoked exclusively through the command palette).
- Keep agent creation and other blocking git/provider work in background workers.
- The PTY approach is CLI-agnostic: any terminal command can be used as a provider. Keep it generic.

## Keeping Documentation In Sync

- When adding, removing, or changing keybindings, update `README.md` if it documents controls or shortcuts.
- When changing features, pane layouts, config paths, or provider behavior, verify that `README.md` still accurately describes the application.
- The README intentionally omits a full keybinding reference to avoid staleness. Do not add a section listing specific key combinations back to it. The in-app `?` help overlay is the authoritative reference for key combinations. Documenting how to configure keybindings (the `[keys]` config format, syntax, and examples) is acceptable — just avoid enumerating individual bindings that would go stale.
- The marketing/documentation site lives in `website/` and is published to `getdux.app` (GitHub Pages). When a change affects anything the site describes or shows — features, command-palette actions, pane layout, install methods, config format, provider defaults, the resource footprint, screenshots, or the install script — update `website/` in the same change so the site stays accurate. Match the site's existing playful tone and never hardcode anything that drifts: avoid counting features/methods in headings and avoid enumerating specific keybindings (the in-app `?` overlay remains authoritative). New copy that references real values (paths, commands, version, RAM usage) should reflect what the app actually does.
- The docs section at `getdux.app/docs` is generated from plain Markdown in `website/docs/`. Add a `.md` file there with `title`, `description`, `group`, and `order` frontmatter and it becomes a page at `/docs/<filename>` automatically — the sidebar (grouped by `group`, ordered by `order`), heading anchors, table of contents, and sitemap entry are all derived for you. When a feature change touches themes, config, providers, or anything a docs page explains, update the relevant Markdown in the same change. Keep prose accurate to the app and avoid enumerating individual keybindings.

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

- Follow the existing `crates/dux-tui/src/app/` submodule pattern when adding new concerns. If a new feature area grows beyond ~200 lines, extract it into its own submodule with `use super::*;` and an `impl App` block.
- Keep changes scoped to one submodule at a time; avoid cross-cutting edits across multiple app submodules in the same PR when possible.
- Use `theme.rs` constants for all colors and styles — never use raw `Color::*` values in rendering code.
- When adding new UI elements, reuse existing semantic color names in `Theme` when they fit. Define a new semantic color only when no existing token fits, and update every supported theme/default mapping in the same change. `theme.rs` is the single source of truth for visual styling.
- The canonical config renderer produces a fully commented config on first creation. Subsequent saves preserve user edits via `toml_edit`. Users can run `dux config diff` to see what changed or `dux config regenerate` to get the latest canonical template.
- When a setting can have a sensible default at first boot (e.g., the user's home directory, platform-specific paths), resolve and store the concrete value in `config.toml` right away — do not leave it commented out or empty. Users should see a working value they can edit, not a placeholder they have to fill in.
- Preserve safe failure behavior around project refresh and failed agent startup.
- **Never use byte-based `.len()` or `[..n]` slicing to truncate user-visible strings.** Terminal output, file paths, and UI text can contain multi-byte UTF-8 characters (box-drawing, block elements, CJK, emoji). Always use `.chars().count()` for length and `.chars().take(n).collect()` (or `char_indices().nth()`) for truncation. Byte-based slicing will panic if the index falls inside a multi-byte character.

## Verification

Use:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

`cargo clippy --all-targets --all-features -- -D warnings` is a CI gate — the `Clippy` check on every pull request runs this exact command and fails the PR if it emits any warning. Run it locally before committing so a toolchain bump or a newly introduced lint doesn't surface only once the PR is pushed. A new stable Rust release can enable lints that previously passed; when that happens, fix the code rather than suppressing the lint unless there is a specific, documented reason.

For interactive smoke testing, ask the user to run `cargo run` as a final sanity check rather than running it automatically.
