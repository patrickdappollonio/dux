// TypeScript types mirroring the dux web server contract.
//
// These shapes must stay in sync with the Rust `ViewModel`, `ServerMessage`,
// and `ClientMessage` definitions on the server side. Server -> client text
// frames are `ServerMessage`; binary frames are raw PTY output bytes. Client
// -> server text frames are `ClientMessage`; binary frames are raw PTY input.

export type SessionStatus = "active" | "detached" | "exited"

// A macro's surface restriction, matching the Rust `MacroSurface` serde casing
// ("agent" | "terminal" | "both"). "agent" macros show only on a focused agent
// pane, "terminal" only on a focused companion terminal, "both" on either.
export type MacroSurface = "agent" | "terminal" | "both"

// A single text macro projected from the server's `[macros]` config, mirroring
// the Rust `MacroView`. Order in `ViewModel.macros` matches the config order.
// `text` is exposed (the web session is authenticated) so the editor dialog can
// show and edit it; the terminal-pane popover runs one via the `run_macro`
// command, which resolves the text + the newline transform engine-side.
export interface MacroView {
  name: string
  text: string
  surface: MacroSurface
}

export interface ProjectView {
  id: string
  name: string
  path: string
  default_provider: string
  explicit_default_provider: string | null
  auto_reopen_agents: boolean | null
  startup_command: string | null
  env: Record<string, string>
  current_branch: string
  branch_status: string
  path_missing: boolean
  /** The project's configured leading/default branch, or null when not detected. */
  leading_branch: string | null
  /** RFC 3339 timestamp of when the project was added, or "" when no store row
   * exists yet. */
  created_at: string
}

export interface PrView {
  number: number
  state: "open" | "merged" | "closed"
  title: string
  url: string
}

export interface TerminalView {
  id: string
  label: string
  has_output: boolean
  /** The command running in the terminal's foreground, or null when the shell
   * itself is idle. Refreshed by the engine at most every ~2s. The displayed
   * terminal title follows this when present, falling back to `label` (see
   * `terminalTitle`). */
  foreground_cmd: string | null
}

export interface SessionView {
  id: string
  project_id: string
  title: string | null
  provider: string
  branch_name: string
  worktree_path: string
  status: SessionStatus
  auto_reopen_enabled: boolean
  pr?: PrView
  terminals: TerminalView[]
  has_output: boolean
  /** Hysteresis boolean: the agent emitted PTY output within the last second.
   * Drives the "working" ping-ring animation on the active status badge. */
  working: boolean
  /** RFC 3339 / ISO 8601 creation time. Backs the client-side sort-by commands
   * (sort agents by creation time) that mirror the TUI's palette parity. */
  created_at: string
  /** RFC 3339 / ISO 8601 last-update time. Backs the sort-by-last-update command. */
  updated_at: string
}

export interface DirEntryView {
  path: string
  label: string
  is_git_repo: boolean
}

// A managed-worktree candidate for the "Attach worktree" flow. Only worktrees
// managed by dux are listed; `adoptable` is false (with a `reason`) when the
// worktree already has an agent and can't be attached again.
export interface ProjectWorktreeEntryView {
  worktree_path: string
  branch_name: string
  adoptable: boolean
  reason: string | null
}

// The branch-warning classification for a candidate project path, mirroring
// the server's `BranchWarningView` / `dux_core::worker::BranchWarningKind`.
// `known` names the resolved default branch; `heuristic` means dux can't
// confidently identify the default. Absence (null on the reply) means the repo
// is already on its default branch — no warning.
export type BranchWarningView =
  | { kind: "known"; default_branch: string }
  | { kind: "heuristic" }

export interface ChangedFileView {
  status: string
  path: string
  additions: number
  deletions: number
  binary: boolean
}

export interface ChangedFiles {
  staged: ChangedFileView[]
  unstaged: ChangedFileView[]
  /** The session id these lists belong to (the currently watched worktree), or
   * `null` when nothing is watched. The changed-files UI renders these lists
   * only when this matches the locally selected session — otherwise it shows a
   * loading state rather than another session's files (cross-tab safety). */
  watched_session_id: string | null
}

/** Fallback xterm.js scrollback used only for the brief window before the first
 * ViewModel arrives; mirrors the core `agent_scrollback_lines` default
 * (`config.rs`). Keep in sync if the Rust default changes. */
export const DEFAULT_SCROLLBACK_LINES = 10000

/** One project's sessions, grouped for the sidebar. `orphaned` marks a group
 * whose project record is gone (its sessions outlived a removed project); its
 * `name` is then a short id slice. Mirrors `dux_core::sidebar::SidebarGroup`. */
export interface SidebarGroup {
  project_id: string
  name: string
  orphaned: boolean
  path_missing: boolean
  session_ids: string[]
}

/** Core-computed sidebar grouping. `agentless_start`, when non-null, is the
 * index in `groups` where the "projects with no agents" section begins.
 * Mirrors `dux_core::sidebar::SidebarModel`. */
export interface SidebarModel {
  groups: SidebarGroup[]
  agentless_start: number | null
}

export interface ViewModel {
  projects: ProjectView[]
  sessions: SessionView[]
  /** Core-computed sidebar grouping (projects + sessions, orphans surfaced) so
   * both surfaces render an identical tree without re-deriving grouping. */
  sidebar: SidebarModel
  changed_files: ChangedFiles
  global_env: Record<string, string>
  available_providers: string[]
  welcome_tips: string[]
  /** Mirrors the binary's display version ('vX.Y.Z' or 'development'); shown in the sidebar brand block. */
  dux_version: string
  randomize_agent_names_by_default: boolean
  /** Whether the new-agent-from-PR flow is available (GitHub integration on +
   * `gh` installed and authenticated). The "From PR" mode is disabled with a
   * quiet explanation when false. */
  gh_available: boolean
  /** Mirrors `config.ui.pr_banner_position` ("top" | "bottom"). Desktop places
   * the PR banner lane above the terminal when "top" and below when "bottom".
   * Mobile ignores this and always renders the banner on top. */
  pr_banner_position: "top" | "bottom"
  /** Mirrors `config.ui.agent_scrollback_lines`. Each xterm.js instance is
   * sized to this so the reconnect repaint's replayed history isn't trimmed by
   * xterm's 1000-line default. */
  agent_scrollback_lines: number
  /** Mirrors `config.ui.show_changes_pane`. Desktop hides the right-hand
   * Changes pane when false; a runtime palette/menu toggle overrides it per
   * session. Optional: older servers omit it, so the web treats it as true. */
  show_changes_pane?: boolean
  /** Surface-aware command-palette commands the web renders as a global
   * "Commands" group, in canonical registry order. Derived from the Rust
   * `dux_core::palette` (the Web/Both subset). Each `id` is the dashed command
   * name; `paletteRegistry` maps it to a store handler. */
  palette_commands: PaletteCommandView[]
  /** Text macros from `[macros]` in `config.toml`, in config order. The
   * terminal-pane popover filters these by the focused target's surface and
   * runs one via the `run_macro` command; the macro-editor dialog lists/edits
   * them. Required, but `store.onViewModel` normalizes a missing key to `[]`
   * (an older snapshot predating the field) so this is always a real array. */
  macros: MacroView[]
}

export interface PaletteCommandView {
  /** Dashed command id (e.g. "sort-agents-by-updated"). */
  id: string
  /** One-line description shown alongside the id. */
  description: string
}

export interface CommandStatus {
  tone: string
  message: string
}

// Server -> client JSON text frames, tagged by `type`.
export type ServerMessage =
  | { type: "view_model"; data: ViewModel }
  | { type: "command_result"; status: CommandStatus | null; error: string | null }
  | { type: "subscribed"; session_id: string }
  | { type: "terminal_created"; session_id: string; terminal_id: string }
  | { type: "error"; message: string }
  | { type: "status"; tone: string; message: string }
  | { type: "commit_message"; session_id: string; message: string }
  | {
      type: "commit_message_snapshot"
      session_id: string
      message: string
    }
  | {
      type: "dir_entries"
      path: string
      entries: DirEntryView[]
      error: string | null
    }
  | { type: "agent_name"; name: string }
  | {
      type: "project_worktrees"
      project_id: string
      entries: ProjectWorktreeEntryView[]
      error: string | null
    }
  | {
      type: "project_path_inspection"
      path: string
      current_branch: string | null
      warning: BranchWarningView | null
      error: string | null
    }

// Argument shapes for the macro `command` frames (sent via `socket.sendCommand`,
// which wraps them in `{ type: "command", command, args }`). They mirror the
// Rust `WireCommand::RunMacro` / `WireCommand::UpdateMacros` payloads. The
// server is authoritative: it resolves `run_macro`'s text + surface gate +
// newline transform engine-side, and validates `update_macros` (empty/duplicate
// names, empty text, unknown surface) before persisting wholesale.
export interface RunMacroArgs {
  target_id: string
  name: string
}

export interface UpdateMacrosArgs {
  entries: MacroView[]
}

// Client -> server JSON text frames, tagged by `type`.
export type ClientMessage =
  | { type: "command"; command: string; args: Record<string, unknown> }
  | { type: "subscribe"; session_id: string }
  | { type: "subscribe_terminal"; terminal_id: string }
  | { type: "create_terminal"; session_id: string }
  | { type: "resize"; session_id: string; rows: number; cols: number }
  | { type: "browse_dir"; path: string | null }
  | { type: "generate_agent_name" }
  | { type: "list_project_worktrees"; project_id: string }
  | { type: "inspect_project_path"; path: string }

export type ConnState = "connecting" | "open" | "closed" | "failed"
