// TypeScript types mirroring the dux web server contract.
//
// These shapes must stay in sync with the Rust `ViewModel`, `ServerMessage`,
// and `ClientMessage` definitions on the server side. Server -> client text
// frames are `ServerMessage`; binary frames are raw PTY output bytes. Client
// -> server text frames are `ClientMessage`; binary frames are raw PTY input.

export type SessionStatus = "active" | "detached" | "exited"

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
}

export interface DirEntryView {
  path: string
  label: string
  is_git_repo: boolean
}

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

export type ConnState = "connecting" | "open" | "closed" | "failed"
