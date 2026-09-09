// TypeScript types mirroring the dux web server contract. These shapes must stay
// in sync with the Rust view/event definitions on the server side.

import type { DropPasteProfile } from "@/lib/fileDrop"
import type { AgentWorkspaceWire } from "@/lib/agentWorkspace"
import type { TerminalOwnerWire } from "@/lib/terminalOwner"

export type SessionStatus = "active" | "detached" | "exited"

// A macro's surface restriction, matching the Rust `MacroSurface` serde casing:
// "agent" shows only on a focused agent pane, "terminal" only on a focused
// companion terminal, "both" on either.
export type MacroSurface = "agent" | "terminal" | "both"

// A text macro projected from the server's `[macros]` config, in config order.
// `text` is exposed because the web session is authenticated; its newline
// transform is applied client-side (`macroPayloadBytes`).
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
  /** True when this PR was manually attached rather than autodetected from the
   * branch name. Drives the agent menu's attach-label flip; detach is offered on
   * any association, so it is gated on the PR's presence rather than on this. */
  overridden: boolean
}

/** How a tab's last run ended, recorded at the moment it ended. An unrecognised
 * `ending` must fall back to the generic sentence rather than being printed. */
export interface TabRunVerdict {
  ending: string
  /** The exit status, for `exited` only. Null on every other kind, and a caller
   * must never print a fallback number the server did not report. */
  status?: number | null
  /** The spawn error, for `launch_failed` only. */
  error?: string | null
  ended_seconds_ago: number
  excerpt: string[]
}

// One provider tab of an agent, in creation order, sharing the agent's worktree.
// Which tab holds the session slot is named by `SessionView.slot_tab_id`, read
// through `isFirstTab`: never a position or a session id, and resume eligibility
// is likewise decided at launch rather than by position. A dormant tab
// (`has_live_process` false) must render without opening the PTY socket, because
// subscribing force-launches the provider server-side.
export interface AgentTabView {
  id: string
  provider: string
  order: number
  working: boolean
  /** This tab is streaming keystroke-level input, a finer cue than `working`.
   * Rolled up any-tab into `SessionView.typing`; missing normalizes to `false`. */
  typing: boolean
  /** This tab needs attention (a permission prompt, a finished turn) the user has
   * not yet looked at. Rolled up any-tab into `SessionView.needs_attention`. */
  needs_attention: boolean
  has_output: boolean
  has_live_process: boolean
  /** This tab's last run ended badly: a launch that failed, or a non-zero exit.
   * Only meaningful while `has_live_process` is false, where it tells apart a
   * dormant tab selecting should start from one that waits for an explicit press.
   * Memory-only server-side; missing reads as "no failure recorded". */
  last_run_failed?: boolean
  /** Why the last run ended badly, when, and its last lines on screen: what the
   * dormant card prints instead of "something went wrong". Present exactly when
   * `last_run_failed` is true, null on a healthy tab, and both null and absent
   * fall back to the generic sentence. */
  last_run_verdict?: TabRunVerdict | null
  /** What this tab's live process launched with, for a file dropped onto its
   * pane: the paste form and the command that identifies the receiving CLI.
   * Absent when no process is live, falling back to `bootstrap.provider_drop_paste`
   * by provider name. It rides the spine rather than the bootstrap document
   * because it changes when a process launches or terminates, which is what
   * refreshes the spine. See `dragDropPasteFormFor`. */
  drop_paste?: DropPasteProfile
  /** The PTY-socket connection id currently input-owning this tab's PTY, or
   * absent when nobody does. Same id space as the `pty.owner` events' `owner`
   * field and the PTY socket's own `connected` frame id, never the events-socket
   * `X-Connection-Id`. The spine is one shared document, so it publishes the
   * identity and each client compares it against its own `ownPtyConnIds` (see
   * `sessionActiveElsewhere`). */
  input_owner?: string
}

export interface TerminalView {
  id: string
  /** Who owns this terminal, tagged. Switch on it with the helpers in
   * `lib/terminalOwner.ts`, never with a two-way conditional. */
  owner: TerminalOwnerWire
  label: string
  has_output: boolean
  /** The terminal emitted PTY output within the last second (hysteresis boolean,
   * mirroring `SessionView.working`). Missing normalizes to `false`. */
  working: boolean
  /** The terminal is streaming keystroke-level input, a finer cue than `working`.
   * Missing normalizes to `false`. */
  typing: boolean
  /** The PTY-socket connection id currently input-owning this terminal's PTY,
   * or absent when nobody does. The exact mirror of `AgentTabView.input_owner`,
   * and read the same way. */
  input_owner?: string
  /** The command running in the terminal's foreground, or null when the shell
   * itself is idle; refreshed by the engine at most every ~2s. The displayed
   * title follows it when present, falling back to `label` (`terminalTitle`). */
  foreground_cmd: string | null
  /** The terminal's manual (drag) display position within the flat Terminals
   * section, ascending. Stamped at spawn so the default equals creation order,
   * rewritten only by a reorder. Runtime only: never persisted, and it resets to
   * creation order on restart. Missing normalizes to `0`. */
  sort_order: number
  /** RFC 3339 spawn time, immutable after spawn, in the same representation as
   * `SessionView`'s timestamps. Missing normalizes to "". */
  created_at: string
  /** RFC 3339 timestamp of the terminal's last PTY activity, falling back to the
   * spawn time when there has been none. Missing normalizes to "". */
  updated_at: string
}

export interface SessionView {
  id: string
  title: string | null
  provider: string
  /** Where this agent lives and what dux may do there: a working copy dux
   * created and owns, or a folder the user already had. Tagged, so every git
   * field lives inside the managed shape and a standalone agent carries no empty
   * strings a screen could mistake for a branch. Every decision it drives goes
   * through `lib/agentWorkspace.ts`, whose switches end in `assertNever`. */
  workspace: AgentWorkspaceWire
  status: SessionStatus
  auto_reopen_enabled: boolean
  pr?: PrView
  /** True while the user has detached this agent's pull request, which stops dux
   * looking for one until they attach a PR by hand or resume detection. Lives on
   * the session rather than on `pr` because it describes the state where there is
   * no PR. Absent reads as false. */
  pr_autodetect_suppressed?: boolean
  /** The id of this agent's session-slot tab. Closing it hands the slot to the
   * next tab in strip order, so this pointer moves; an agent's only tab cannot be
   * closed. Read it through `isFirstTab` in `lib/agentTabs.ts` rather than
   * comparing a tab id against the session id. Missing is filled with the session
   * id, the placeholder for "the first tab, whichever it is". */
  slot_tab_id: string
  /** The agent's provider tabs in creation order; a session always has at least
   * one, and the strip renders only when there are two or more. Missing coerces
   * to `[]` at ingestion. */
  tabs: AgentTabView[]
  has_output: boolean
  /** Hysteresis boolean: the agent emitted PTY output within the last second.
   * Drives the "working" ping-ring animation on the active status badge. */
  working: boolean
  /** Any of the agent's tabs is streaming keystroke-level input, a finer cue than
   * `working`, rolled up any-tab. Missing coerces to `false`. */
  typing: boolean
  /** Any of the agent's tabs needs attention (a permission prompt, a finished
   * turn) the user has not yet looked at, rolled up any-tab. Drives the sidebar
   * dot, the browser-tab count and the favicon dot; missing coerces to `false`. */
  needs_attention: boolean
  /** RFC 3339 / ISO 8601 creation time. Backs the sort-by-creation command. */
  created_at: string
  /** RFC 3339 / ISO 8601 last-update time. Backs the sort-by-last-update command. */
  updated_at: string
  /** The tab id the user last focused on this agent, restored when navigating
   * away and back. Null, absent, or naming a tab no longer in `tabs` means "no
   * memory": resolve it through `resolveFocusedTab` in `lib/agentTabs.ts`. An
   * explicit deep link (`#/agent/:id/tab/:t`) always wins over it. */
  last_focused_tab?: string | null
}

/** One startup-command log file, shared by both scopes of the viewer: one
 * agent's runs, and every run across a project's agents. */
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

/** One scope's startup-command log listing, newest first, with the newest file's
 * contents pre-loaded (`selected` is null when the scope has no logs yet). Both
 * scopes return this same shape, which is what lets one dialog serve both. */
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

// A managed-worktree candidate for the "Attach worktree" flow. `adoptable` is
// false (with a `reason`) when the worktree already has an agent.
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
  // Whether the worktree holds uncommitted work (staged, unstaged or untracked).
  // Removal is forced and there is no trash, so the confirmation says so.
  dirty: boolean
  // The agent holding a non-adoptable worktree; its display name is resolved
  // client-side from the spine (`title || branch_name`).
  agent_id: string | null
}

// The branch-warning classification for a candidate project path. `known` names
// the resolved default branch; `heuristic` means dux cannot confidently identify
// it. Null on the reply means the repo is already on its default branch.
export type BranchWarningView =
  | { kind: "known"; default_branch: string }
  | { kind: "heuristic" }

// How an inspected add-project candidate path classifies: "repo" (work-tree
// root), "bare" (bare root), "repo_subdir" (inside a repo or git's internal
// directory; blocked), or "plain" (not a repo; dux offers to initialize one). A
// missing kind is treated as "repo".
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
   * `null` when nothing is watched. The UI renders the lists only when this
   * matches the locally selected session, so one tab never shows another
   * session's files. */
  watched_session_id: string | null
}

/** Fallback xterm.js scrollback for the window before the first ViewModel
 * arrives. Must match the core `agent_scrollback_lines` default in `config.rs`. */
export const DEFAULT_SCROLLBACK_LINES = 10000

/** One project's sessions, grouped for the sidebar. `orphaned` marks a group
 * whose project record is gone; its `name` is then a short id slice. */
export interface SidebarGroup {
  project_id: string
  name: string
  orphaned: boolean
  path_missing: boolean
  session_ids: string[]
}

/** Core-computed sidebar grouping. `agentless_start`, when non-null, is the
 * index in `groups` where the "projects with no agents" section begins. */
export interface SidebarModel {
  groups: SidebarGroup[]
  agentless_start: number | null
}

// The broadcast frame, carrying only `changed_files`. The data itself is owned
// by the store's `changes` slice; this type exists to mirror the wire frame.
export interface ViewModel {
  changed_files: ChangedFiles
}

export type ConnState = "connecting" | "open" | "closed" | "failed"

// --- /ws/events channel ----------------------------------------------------
//
// The only JSON socket. The client manages a per-connection interest set; the
// server pushes resource-change notifications plus control frames. Every frame
// is a flat object discriminated by `event`.

// Server -> client resource-change frame. `id` scopes it to one resource; `rev`
// is the monotonic per-session revision the client compares against its
// last-applied rev.
export interface ResourceEvent {
  event: string
  id?: string
  rev?: number
}

// Server -> client `/ws/events` frame: one flat shape discriminated by `event`,
// covering resource changes, the `workspace` push that carries its own value, and
// control frames. Terminal add/remove/relabel folds into `sessions.changed`;
// there is no `terminals.changed`. Fields beyond `event` are optional so one
// handler can switch on `event` and read only what that frame carries.
export interface EventsServerMessage {
  event: string
  /** Resource id (`session.changes`)
   *  OR the per-connection id (`connected`). */
  id?: string
  /** Monotonic per-session revision (`session.changes`), or the workspace
   *  document's revision (`workspace`). */
  rev?: number
  /** The whole workspace document (`workspace`): the same bytes
   *  `GET /api/v1/workspace` returns. Typed as `unknown` because the shape check
   *  belongs at the one ingestion boundary, `normalizeWorkspace`. */
  workspace?: unknown
  /** The claiming connection's id on a `pty.owner` handover. A client compares it
   *  against its own PTY-socket connection id to decide ownership: own id = owner,
   *  foreign id = read-only placeholder. */
  owner?: string
  /** The monotonic ownership epoch on a `pty.owner` handover, reflecting true
   *  claim order. The client keeps only the highest epoch seen per pty, so a
   *  reordered broadcast cannot resurrect a stale owner. */
  epoch?: number
  /** The claiming connection's raw `User-Agent` on a `pty.owner` handover, parsed
   *  client-side into a human label for the take-over placeholder. Absent for
   *  every other event and when the claimer sent none. */
  device?: string
  /** Status correlation key (`status`/`status_cleared`); null/absent = the
   *  anonymous slot. */
  key?: string | null
  /** Status tone (`status`): "busy" | "info" | "warning" | "error". */
  tone?: string
  /** Status message (`status`). */
  message?: string
  /** Whether this status waits for the user instead of for a clock (`status`).
   *  Absent reads as `false`, never as "unknown, better keep it on screen". What
   *  earns it is documented on `NotifyOptions.sticky` in `lib/notify.ts`. */
  sticky?: boolean
  /** Status scope (`status`): the literal `"all"` for a workspace broadcast, or
   *  `{connection: "<id>"}` for one addressed to a single connection (the
   *  serialized shape, so this is not a plain string). Read in
   *  `lib/statusRouting.ts`, where the standalone editor tab renders addressed
   *  statuses only and stays quiet for broadcasts. */
  scope?: string | { connection: string }
}

// Client -> server interest frames. Topics are opaque strings, app-wide
// ("sessions") or per-resource ("session:<id>:changes"); the server accepts both
// keys in one frame.
export interface EventsClientMessage {
  subscribe?: string[]
  unsubscribe?: string[]
}
