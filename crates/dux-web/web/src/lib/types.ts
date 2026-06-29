// TypeScript types mirroring the dux web server contract.
//
// These shapes must stay in sync with the Rust view/event definitions on the
// server side. Since Phase 6 the legacy `/ws` control socket is gone: every
// data read/mutation is a REST `/api/v1/*` call and every server push (resource
// changes plus status/connection control frames) rides `/ws/events`. PTY byte
// I/O rides the dedicated per-PTY sockets (`/ws/sessions/:id/pty` and
// `/ws/sessions/:id/terminals/:tid/pty`) — see `lib/ptySocket.ts`.

export type SessionStatus = "active" | "detached" | "exited"

// A macro's surface restriction, matching the Rust `MacroSurface` serde casing
// ("agent" | "terminal" | "both"). "agent" macros show only on a focused agent
// pane, "terminal" only on a focused companion terminal, "both" on either.
export type MacroSurface = "agent" | "terminal" | "both"

// A single text macro projected from the server's `[macros]` config, mirroring
// the Rust `MacroView`. Order matches the config order. `text` is exposed (the
// web session is authenticated) so the editor dialog can show/edit it and the
// terminal-pane popover can write it straight to the focused PTY socket (Phase 5
// applies the newline transform client-side; see `runMacro`/`macroPayloadBytes`).
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

/** One startup-command log file for an agent (see `GET /sessions/:id/startup-logs`). */
export interface StartupLogEntry {
  name: string
  /** RFC 3339 last-modified time, or null when unavailable. */
  modified_at: string | null
}

/** A startup-command log file's name + full contents. */
export interface StartupLogContent {
  name: string
  content: string
}

/** The startup-command log listing for an agent: every log file (newest first)
 * plus the newest file's contents pre-loaded (`selected` is null when there are
 * no logs yet). */
export interface StartupLogsList {
  entries: StartupLogEntry[]
  selected: StartupLogContent | null
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

// The broadcast ViewModel is now a residual frame carrying ONLY `changed_files`.
// The eleven build-static / config-derived fields (providers, macros, palette
// commands, welcome tips, version, randomize default, gh availability, PR banner
// position, scrollback, changes-pane default, global env) moved to
// `GET /api/v1/bootstrap` (`bootstrapApi.ts`, invalidated by `config.changed`),
// and the projects/sessions/sidebar fields moved to `GET /api/v1/spine`
// (`spineApi.ts`, invalidated by `projects.changed`/`sessions.changed`) — neither
// belonged on a per-change broadcast. The changed-files data itself is owned by
// the store's `changes` slice (`GET /api/v1/sessions/:id/changes`); this field
// remains on the type only to mirror the residual wire frame.
export interface ViewModel {
  changed_files: ChangedFiles
}

export interface PaletteCommandView {
  /** Dashed command id (e.g. "sort-agents-by-updated"). */
  id: string
  /** One-line description shown alongside the id. */
  description: string
}

export type ConnState = "connecting" | "open" | "closed" | "failed"

// --- /ws/events channel ----------------------------------------------------
//
// Since Phase 6 the ONLY JSON socket (the legacy `/ws` is gone). The client
// manages a per-connection interest set; the server pushes resource-change
// notifications plus the control frames the old `/ws` used to carry. Every frame
// is a flat object discriminated by `event`.

// Server -> client resource-change frame. `event` is the resource discriminator
// (e.g. "session.changes"); `id` scopes it to one resource (the session id);
// `rev` is the monotonic per-session revision the client compares against its
// last-applied rev. Lag catch-up arrives as an ordinary `session.changes`
// written directly to this connection, so the same handler covers it.
export interface ResourceEvent {
  event: string
  id?: string
  rev?: number
}

// Server -> client `/ws/events` frame. A single flat shape (the server emits a
// flat JSON object) discriminated by `event`:
//   - resource changes: `session.changes` (id+rev), `projects.changed`,
//     `sessions.changed`, `config.changed`
//     (terminal add/remove/relabel folds into `sessions.changed`; there is no
//     separate `terminals.changed` frame);
//   - control frames migrated off the retired `/ws`: `connected` (id = the
//     per-connection id echoed via `X-Connection-Id`), `status`
//     (key?/tone/message, plus a server-side `scope` the client ignores), and
//     `status_cleared` (key?).
// Fields beyond `event` are optional so one handler can switch on `event` and
// read only the fields that frame carries.
export interface EventsServerMessage {
  event: string
  /** Resource id (`session.changes`)
   *  OR the per-connection id (`connected`). */
  id?: string
  /** Monotonic per-session revision (`session.changes`). */
  rev?: number
  /** The claiming connection's id on a `pty.owner` handover. A client viewing
   *  that PTY compares it against its own PTY-socket connection id to decide
   *  ownership definitively (own id = owner, foreign id = read-only placeholder). */
  owner?: string
  /** The monotonic ownership epoch on a `pty.owner` handover, assigned under the
   *  server's owners lock so it reflects true claim order. The client keeps only
   *  the highest epoch seen per pty and ignores any older arrival, so a reordered
   *  broadcast cannot resurrect a stale owner. */
  epoch?: number
  /** Status correlation key (`status`/`status_cleared`); null/absent = the
   *  anonymous slot. */
  key?: string | null
  /** Status tone (`status`): "busy" | "info" | "warning" | "error". */
  tone?: string
  /** Status message (`status`). */
  message?: string
  /** Server-side status scope (`status`); already scope-filtered by the server,
   *  so the client ignores it. */
  scope?: string
}

// Client -> server interest frames. Topics are opaque strings: coarse app-wide
// topics ("sessions", "projects", "config") and fine per-resource topics
// ("session:<id>:changes"). The server accepts both keys in one frame, so this
// is a single shape with optional `subscribe`/`unsubscribe` arrays.
export interface EventsClientMessage {
  subscribe?: string[]
  unsubscribe?: string[]
}
