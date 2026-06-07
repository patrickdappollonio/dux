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
}

export type DiffLineKind = "context" | "insert" | "delete"

export interface DiffLine {
  kind: DiffLineKind
  old_line: number | null
  new_line: number | null
  content: string
}

export interface DiffHunk {
  header: string
  lines: DiffLine[]
}

export interface FileDiff {
  path: string
  binary: boolean
  unchanged: boolean
  old_size: number
  new_size: number
  hunks: DiffHunk[]
}

export interface ViewModel {
  projects: ProjectView[]
  sessions: SessionView[]
  changed_files: ChangedFiles
  global_env: Record<string, string>
  available_providers: string[]
  welcome_tips: string[]
  randomize_agent_names_by_default: boolean
  /** Whether the new-agent-from-PR flow is available (GitHub integration on +
   * `gh` installed and authenticated). The "From PR" mode is disabled with a
   * quiet explanation when false. */
  gh_available: boolean
  /** Mirrors `config.ui.pr_banner_position` ("top" | "bottom"). Desktop places
   * the PR banner lane above the terminal when "top" and below when "bottom".
   * Mobile ignores this and always renders the banner on top. */
  pr_banner_position: "top" | "bottom"
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
  | { type: "commit_message"; message: string }
  | {
      type: "diff"
      session_id: string
      path: string
      diff: FileDiff | null
      error: string | null
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
  | { type: "get_diff"; session_id: string; path: string }
  | { type: "browse_dir"; path: string | null }
  | { type: "generate_agent_name" }
  | { type: "list_project_worktrees"; project_id: string }
  | { type: "inspect_project_path"; path: string }

export type ConnState = "connecting" | "open" | "closed" | "failed"
