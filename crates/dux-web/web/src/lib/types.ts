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
  current_branch: string
  branch_status: string
  path_missing: boolean
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

export interface ViewModel {
  projects: ProjectView[]
  sessions: SessionView[]
  changed_files: ChangedFiles
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
  | { type: "error"; message: string }

// Client -> server JSON text frames, tagged by `type`.
export type ClientMessage =
  | { type: "command"; command: string; args: Record<string, unknown> }
  | { type: "subscribe"; session_id: string }
  | { type: "resize"; session_id: string; rows: number; cols: number }

export type ConnState = "connecting" | "open" | "closed" | "failed"
