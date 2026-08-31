// TypeScript types mirroring the dux web server contract.
//
// These shapes must stay in sync with the Rust view/event definitions on the
// server side. Every data read/mutation is a REST `/api/v1/*` call and every
// server push (resource
// changes plus status/connection control frames) rides `/ws/events`. PTY byte
// I/O rides the dedicated per-PTY sockets (`/ws/sessions/:id/pty` and
// `/ws/sessions/:id/terminals/:tid/pty`) — see `lib/ptySocket.ts`.

import type { DropPasteProfile } from "@/lib/fileDrop"
import type { AgentWorkspaceWire } from "@/lib/agentWorkspace"
import type { TerminalOwnerWire } from "@/lib/terminalOwner"

export type SessionStatus = "active" | "detached" | "exited"

// A macro's surface restriction, matching the Rust `MacroSurface` serde casing
// ("agent" | "terminal" | "both"). "agent" macros show only on a focused agent
// pane, "terminal" only on a focused companion terminal, "both" on either.
export type MacroSurface = "agent" | "terminal" | "both"

// A single text macro projected from the server's `[macros]` config, mirroring
// the Rust `MacroView`. Order matches the config order. `text` is exposed (the
// web session is authenticated) so the editor dialog can show/edit it and the
// terminal-pane popover can write it straight to the focused PTY socket (the
// newline transform is applied client-side; see `runMacro`/`macroPayloadBytes`).
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
  /** True when this PR was manually attached (pinned) rather than autodetected
   * from the branch name. Lives on the PR view, not the session, so
   * "overridden without a PR" is unrepresentable. Drives the agent menu's
   * attach-label flip. Detach is offered on ANY association, pinned or not, so
   * it is gated on the PR's presence rather than on this. */
  overridden: boolean
}

// One provider tab of an agent, mirroring the Rust `AgentTabView`. Tabs are
// generic provider sessions in the agent's shared worktree, in creation order
// (`tabs[0]` is the session-slot tab, the one named by `SessionView.slot_tab_id`
// — no tab is privileged). Resume is decided dynamically at launch: a tab
// resumes the worktree's prior conversation only when it is the sole tab coming up
// (no other tab live/launching); concurrent tabs start fresh. `has_live_process`
// is false for a tab with no running PTY (a tab reopened dormant after a restart)
// — the web must render its dormant card WITHOUT opening the PTY socket, because
// subscribing force-launches the provider server-side.
export interface AgentTabView {
  id: string
  provider: string
  order: number
  working: boolean
  /** This specific tab is streaming keystroke-level input (the provider is
   * "typing"), a finer-grained cue than `working`. Rolled up any-tab into
   * `SessionView.typing`. An older server omits it; `normalizeWorkspace` normalizes a
   * missing value to `false`. */
  typing: boolean
  /** This specific tab needs attention (a permission prompt, a finished turn)
   * the user has not yet looked at. The tab strip marks the flagged pill; the
   * sidebar rolls this up across tabs into `SessionView.needs_attention`. */
  needs_attention: boolean
  has_output: boolean
  has_live_process: boolean
  /** What this tab's LIVE process launched with, for a file dropped onto its
   * pane: the paste form and the command that identifies the receiving CLI.
   * Absent when no process is live (a dormant tab), and absent on an older
   * server; both fall back to `bootstrap.provider_drop_paste` by provider name.
   *
   * It rides the SPINE rather than the bootstrap document because it changes
   * when a process LAUNCHES or TERMINATES, and the spine is what those refresh
   * (`sessions.changed`). Published on the bootstrap document (refreshed by
   * `config.changed`) the browser's copy went stale for the whole life of a
   * process, so a tab relaunched under a different provider was still quoted for
   * the previous one until a reconnect. See `dragDropPasteFormFor`. */
  drop_paste?: DropPasteProfile
  /** The PTY-socket connection id currently input-owning this tab's PTY, or
   * absent when nobody does (nobody claimed it, the owner disconnected, or the
   * server predates the field). Same id space as the `pty.owner` events'
   * `owner` field and the PTY socket's own `connected` frame id — NOT the
   * events-socket `X-Connection-Id`. The spine publishes the IDENTITY rather
   * than a per-client "elsewhere" flag because it is one shared document:
   * each client compares against its own live PTY-socket ids
   * (`ownPtyConnIds`) in `sessionActiveElsewhere`, which is how a hub or
   * sidebar row menu can gate an agent this device never attached to. */
  input_owner?: string
}

export interface TerminalView {
  id: string
  /** Who owns this terminal, tagged. Every terminal carries its own owner now
   * that they arrive as ONE flat `Spine.terminals` collection rather than nested
   * inside the session or project that owns them. Switch on it with the helpers
   * in `lib/terminalOwner.ts`, never with a two-way conditional. */
  owner: TerminalOwnerWire
  label: string
  has_output: boolean
  /** The terminal emitted PTY output within the last second (hysteresis boolean,
   * mirroring `SessionView.working`). Drives the terminal row's "Working" state
   * word and its working cue. An older server omits it; `normalizeWorkspace` normalizes
   * a missing value to `false`. */
  working: boolean
  /** The terminal is streaming keystroke-level input ("typing"), a finer cue than
   * `working`. Drives the terminal row's "Typing" state word and typing caret. An
   * older server omits it; `normalizeWorkspace` normalizes a missing value to `false`. */
  typing: boolean
  /** The PTY-socket connection id currently input-owning this terminal's PTY,
   * or absent when nobody does. The exact mirror of `AgentTabView.input_owner`,
   * and read the same way: it is what lets a terminal's take-over card notice
   * that the device it is naming stopped driving while the events socket was
   * down. */
  input_owner?: string
  /** The command running in the terminal's foreground, or null when the shell
   * itself is idle. Refreshed by the engine at most every ~2s. The displayed
   * terminal title follows this when present, falling back to `label` (see
   * `terminalTitle`). */
  foreground_cmd: string | null
  /** The terminal's manual (drag) display position within the flat Terminals
   * section, ascending. Stamped at spawn from a monotonic counter so the default
   * equals creation order, and rewritten only by a reorder. RUNTIME ONLY: it is
   * never persisted and resets to creation order on restart. An older server omits
   * it; `normalizeWorkspace` normalizes a missing value to `0`. */
  sort_order: number
  /** RFC 3339 spawn time, immutable after spawn. Same representation as
   * `SessionView`'s timestamps so the terminal sort can compute the same
   * "recently created" order as the agent sort. An older server omits it;
   * `normalizeWorkspace` normalizes a missing value to "". */
  created_at: string
  /** RFC 3339 timestamp of the terminal's last PTY activity (falling back to the
   * spawn time when there has been none). Drives the "recently updated" sort. An
   * older server omits it; `normalizeWorkspace` normalizes a missing value to "". */
  updated_at: string
}

export interface SessionView {
  id: string
  title: string | null
  provider: string
  /** Where this agent lives and what dux may do there: a working copy dux
   * created and owns, or a folder the user already had. TAGGED, so every git
   * field lives inside the managed shape and a standalone agent carries no
   * empty strings a screen could mistake for a branch.
   *
   * Every decision it drives goes through `lib/agentWorkspace.ts`, whose
   * switches end in `assertNever`. `normalizeWorkspace` synthesizes the managed
   * shape for an older server that still sends the flat git fields. */
  workspace: AgentWorkspaceWire
  status: SessionStatus
  auto_reopen_enabled: boolean
  pr?: PrView
  /** True while the user has detached this agent's pull request, which stops
   * dux looking for one on it until they attach a PR by hand or resume
   * detection. Lives on the session and not on `pr` (unlike `overridden`)
   * precisely because it describes the state where there IS no PR: it is what
   * gates the menu's "Resume PR autodetection" way back. An older server omits
   * it, which reads as false. */
  pr_autodetect_suppressed?: boolean
  /** The id of this agent's session-slot tab: its first tab, the one the user
   * cannot close. Read it through `isFirstTab` in `lib/agentTabs.ts` rather than
   * comparing a tab id against the session id. An older server omits it;
   * `normalizeWorkspace` fills it with the session id, which is what such a
   * server meant by it. */
  slot_tab_id: string
  /** The agent's provider tabs in creation order (`tabs[0].id === slot_tab_id`). A session
   * always has at least one tab; the tab strip renders only when there are two or
   * more. See `AgentTabView`. An older server that predates tabs (e.g. after a
   * binary downgrade, seen by an already-open client) omits the field;
   * `normalizeWorkspace` coerces a missing value to `[]` at ingestion. */
  tabs: AgentTabView[]
  has_output: boolean
  /** Hysteresis boolean: the agent emitted PTY output within the last second.
   * Drives the "working" ping-ring animation on the active status badge. */
  working: boolean
  /** Any of the agent's tabs is streaming keystroke-level input ("typing"), a
   * finer-grained cue than `working`, rolled up any-tab. Drives the row's
   * "Typing" state word and typing caret. An older server omits it;
   * `normalizeWorkspace` coerces a missing value to `false`. */
  typing: boolean
  /** Any of the agent's tabs needs attention (a permission prompt, a finished
   * turn) the user has not yet looked at. Rolled up any-tab, mirroring `working`.
   * Drives the sidebar dot, the browser-tab count, and the favicon dot. An older
   * server omits it; `normalizeWorkspace` normalizes a missing value to `false`. */
  needs_attention: boolean
  /** RFC 3339 / ISO 8601 creation time. Backs the client-side sort-by commands
   * (sort agents by creation time) that mirror the TUI's palette parity. */
  created_at: string
  /** RFC 3339 / ISO 8601 last-update time. Backs the sort-by-last-update command. */
  updated_at: string
  /** The tab id the user last focused on this agent, remembered so navigating
   * away and back (sidebar click, or the bare `#/agent/:id` route) restores it.
   * `null`/undefined (or a value naming a tab no longer in `tabs`) means "no
   * memory": resolve to the session-slot tab (`id`). See `resolveFocusedTab` in
   * `lib/agentTabs.ts`. An older server that predates this field omits it;
   * `normalizeWorkspace` normalizes a missing value to `null`. An explicit deep link
   * (`#/agent/:id/tab/:t`) always wins over this — see `restoreDeepLink`. */
  last_focused_tab?: string | null
}

/** One startup-command log file. Shared by both scopes of the viewer: the agent
 * one (`GET /sessions/:id/startup-logs`) and the project one
 * (`GET /projects/:id/startup-logs`, every run across every agent). */
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

/** The startup-command log listing for one scope (an agent, or a whole project):
 * every log file in it, newest first, plus the newest file's contents pre-loaded
 * (`selected` is null when the scope has no logs yet). Both routes return this
 * same shape, which is what lets one dialog serve both. */
export interface StartupLogsList {
  entries: StartupLogEntry[]
  selected: StartupLogContent | null
}

export interface DirEntryView {
  path: string
  label: string
  is_git_repo: boolean
  is_parent: boolean
}

// A managed-worktree candidate for the "Attach worktree" flow. Only worktrees
// managed by dux are listed; `adoptable` is false (with a `reason`) when the
// worktree already has an agent and can't be attached again.
export interface ProjectWorktreeEntryView {
  worktree_path: string
  // The row LABEL: the branch when there is one, else a "detached <sha>"
  // stand-in the server invents for display.
  branch_name: string
  // The real branch, null for a detached worktree. `branch_name` cannot answer
  // "is there a branch here to delete?", so the delete confirmation reads this.
  branch: string | null
  adoptable: boolean
  reason: string | null
  // Whether the worktree holds uncommitted work (staged, unstaged or
  // untracked). Removal is `git worktree remove --force` and there is no trash,
  // so the delete confirmation says so specifically.
  dirty: boolean
  // The agent holding a non-adoptable worktree. The display name is resolved
  // client-side from the spine (`title || branch_name`) so the naming lives in
  // one place.
  agent_id: string | null
}

// The branch-warning classification for a candidate project path, mirroring
// the server's `BranchWarningView` / `dux_core::worker::BranchWarningKind`.
// `known` names the resolved default branch; `heuristic` means dux can't
// confidently identify the default. Absence (null on the reply) means the repo
// is already on its default branch — no warning.
export type BranchWarningView =
  | { kind: "known"; default_branch: string }
  | { kind: "heuristic" }

// How an inspected add-project candidate path classifies, mirroring the
// server's `InspectReply.kind`: "repo" (work-tree root), "bare" (bare root),
// "repo_subdir" (inside a repo, or inside git's internal directory; blocked),
// or "plain" (not a repo; dux offers to initialize one). An older backend
// omits the field entirely; the client treats a missing kind as "repo".
export type InspectKind = "repo" | "bare" | "repo_subdir" | "plain"

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
// The build-static / config-derived fields (providers, macros, welcome tips,
// version, randomize default, gh availability, PR banner position, scrollback,
// changes-pane default, global env) moved to
// `GET /api/v1/bootstrap` (`bootstrapApi.ts`, invalidated by `config.changed`),
// and the projects/sessions/sidebar fields moved to `GET /api/v1/workspace`
// (`workspaceApi.ts`, read at boot and then PUSHED as a `workspace` event on
// every change): neither belonged on a per-change broadcast. The changed-files data itself is owned by
// the store's `changes` slice (`GET /api/v1/sessions/:id/changes`); this field
// remains on the type only to mirror the residual wire frame.
export interface ViewModel {
  changed_files: ChangedFiles
}

export type ConnState = "connecting" | "open" | "closed" | "failed"

// --- /ws/events channel ----------------------------------------------------
//
// The only JSON socket. The client manages a per-connection interest set; the
// server pushes resource-change notifications plus control frames. Every frame
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
//   - the one resource event that CARRIES its value: `workspace` (rev + the
//     whole workspace document). The server holds that document pre-serialized
//     and every tab needs the same bytes, so it is pushed once per change
//     instead of each tab answering `projects.changed`/`sessions.changed` with
//     its own full GET. Those two keep firing for a page too old to read the
//     push;
//   - control frames: `connected` (id = the
//     per-connection id echoed via `X-Connection-Id`), `status`
//     (key?/tone/message, plus a `scope` the standalone editor tab reads to
//     stay quiet for workspace broadcasts), and
//     `status_cleared` (key?).
// Fields beyond `event` are optional so one handler can switch on `event` and
// read only the fields that frame carries.
export interface EventsServerMessage {
  event: string
  /** Resource id (`session.changes`)
   *  OR the per-connection id (`connected`). */
  id?: string
  /** Monotonic per-session revision (`session.changes`), or the workspace
   *  document's revision (`workspace`). */
  rev?: number
  /** The whole workspace document (`workspace`): the same bytes
   *  `GET /api/v1/workspace` returns, pushed on every change so N open tabs do
   *  not each answer a ping with N identical GETs. Typed as `unknown` because
   *  this envelope is the wire and the shape check belongs at the one ingestion
   *  boundary (`normalizeWorkspace`), which has to cope with older servers'
   *  documents anyway. */
  workspace?: unknown
  /** The claiming connection's id on a `pty.owner` handover. A client viewing
   *  that PTY compares it against its own PTY-socket connection id to decide
   *  ownership definitively (own id = owner, foreign id = read-only placeholder). */
  owner?: string
  /** The monotonic ownership epoch on a `pty.owner` handover, assigned under the
   *  server's owners lock so it reflects true claim order. The client keeps only
   *  the highest epoch seen per pty and ignores any older arrival, so a reordered
   *  broadcast cannot resurrect a stale owner. */
  epoch?: number
  /** The claiming connection's raw `User-Agent` on a `pty.owner` handover,
   *  captured server-side (the other device is only known to the server). The
   *  client parses it into a human label ("Chrome on macOS") for the take-over
   *  placeholder. Absent for every other event and when the claimer sent none. */
  device?: string
  /** Status correlation key (`status`/`status_cleared`); null/absent = the
   *  anonymous slot. */
  key?: string | null
  /** Status tone (`status`): "busy" | "info" | "warning" | "error". */
  tone?: string
  /** Status message (`status`). */
  message?: string
  /** Whether this status waits for the user instead of for a clock (`status`).
   *  `WireStatus.sticky` carries `#[serde(default)]`, so a current server sends
   *  the field on EVERY status frame, `true` or `false`. It is optional here
   *  for compatibility in both directions (a server that predates the field,
   *  and a replay recorded before it), and an absent field reads as `false`
   *  rather than as "unknown, better keep it on screen". What earns it is
   *  documented on `NotifyOptions.sticky` in `lib/notify.ts`. */
  sticky?: boolean
  /** Status scope (`status`): the literal `"all"` for a workspace broadcast, or
   *  `{connection: "<id>"}` for one addressed to a single connection (that is
   *  the serialized shape, so this is not a plain string). The server has
   *  already delivered it only where it belongs; the client reads it for one
   *  further decision, in `lib/statusRouting.ts`: the standalone editor tab
   *  renders addressed statuses only and stays quiet for broadcasts. */
  scope?: string | { connection: string }
}

// Client -> server interest frames. Topics are opaque strings: coarse app-wide
// topics ("sessions", "projects", "config") and fine per-resource topics
// ("session:<id>:changes"). The server accepts both keys in one frame, so this
// is a single shape with optional `subscribe`/`unsubscribe` arrays.
export interface EventsClientMessage {
  subscribe?: string[]
  unsubscribe?: string[]
}
