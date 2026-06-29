import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { sanitizeAgentName } from "./agentName"
import { git } from "./git"
import { projectsApi, type PatchProjectBody } from "./projectsApi"
import { sessionsApi, SessionsApiError } from "./sessionsApi"

import { ordersMatch } from "./reorder"
import { sortedSessionIds, type SortKey } from "./sortSessions"
import { EventsSocket } from "./eventsSocket"
import { getActivePtySocket } from "./ptySocket"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "./ptyOwnership"
import { macroPayloadBytes } from "./macros"
import { terminalsApi } from "./terminalsApi"
import { browseApi } from "./browseApi"
import { configApi } from "./configApi"
import { setConnectionId } from "./connection"
import {
  ChangesFetchError,
  fetchChanges,
  type SessionChangesResponse,
} from "./changesApi"
import { type Bootstrap, fetchBootstrap } from "./bootstrapApi"
import { applyFavicon } from "./favicon"
import { resolveInstanceTitle } from "./instanceTitle"
import { type Spine, fetchSpine } from "./spineApi"
import type {
  BranchWarningView,
  ChangedFileView,
  ConnState,
  DirEntryView,
  EventsServerMessage,
  MacroView,
  ProjectWorktreeEntryView,
  StartupLogContent,
  StartupLogEntry,
} from "./types"

// The currently-streamed target: either an agent session or one of its
// companion terminals. Both carry a `sessionId` so session-scoped UI (the
// breadcrumb, changed files) keeps working regardless of which is focused.
export type SelectedTarget =
  | { kind: "agent"; sessionId: string }
  | { kind: "terminal"; terminalId: string; sessionId: string }

// The mobile hub-&-spoke shell shows one screen at a time: the project/session
// hub ("home"), the focused terminal, or the changed-files view. Desktop never
// reads this — it renders all three panes at once.
export type MobileScreen = "home" | "terminal" | "changes"

// The name-input dialog (one component, two modes) targets either a fresh agent
// in a project or a fork of an existing session. The shared draft/randomize/
// generated/pending state below drives both; only the dispatch target differs.
export type CreateAgentTarget =
  | { kind: "new"; projectId: string }
  | { kind: "fork"; sessionId: string }
  | { kind: "pr"; projectId: string }

// The file pending discard-confirmation, or null. `untracked` drives the
// dialog's warning copy (a tracked file is restored from HEAD; an untracked
// file is permanently deleted). Derived from the file's git status at the moment
// the affordance is clicked; the server independently re-derives and re-validates
// it, so this is only a UI hint.
export interface DiscardTarget {
  sessionId: string
  path: string
  untracked: boolean
}

// An optimistic session-order overlay for one project: while a drag-and-drop
// reorder is in flight, the UI renders `ids` (the new complete order of that
// project's sessions) instead of the server's order, so the row doesn't snap
// back during the ≤50ms round-trip. Cleared when a ViewModel arrives whose order
// already matches, or on any error status.
export interface PendingSessionOrder {
  projectId: string
  ids: string[]
}

// The changed-files request state machine for the SELECTED session. This is the
// single source of truth for changed-files data across the app (the changes
// pane, commit dialog, discard dialog, mobile badge, and editor markers all read
// it), fed by `GET /api/v1/sessions/:id/changes` and invalidated by
// `session.changes` events over `/ws/events`.
//
//   - `idle`    nothing selected (or the slice was cleared, e.g. a 404).
//   - `loading` a fetch is in flight for `sessionId`.
//   - `loaded`  `staged`/`unstaged` are current for `sessionId` at `rev`.
//   - `error`   the last fetch failed; `error` carries why. Self-heals on the
//               next `session.changes` event (which always refetches in this
//               state, side-stepping the `rev > undefined` trap).
//
// `sessionId` is the session these lists belong to; consumers only trust the
// slice when it equals their own session id. `rev` is the monotonic per-session
// revision of the applied data; a response or event with an older `rev` is
// dropped (out-of-order / lost-race protection).
export type ChangesPhase = "idle" | "loading" | "loaded" | "error"

export interface ChangesSlice {
  sessionId: string | null
  phase: ChangesPhase
  rev: number
  staged: ChangedFileView[]
  unstaged: ChangedFileView[]
  error: string | null
}

// A tiny external store backed by `useSyncExternalStore`. A single module-level
// `EventsSocket` (`/ws/events`) feeds it: resource-change events plus the
// connection id and status frames (surfaced as sonner toasts). Every action is a
// REST `/api/v1/*` call. The PTY byte stream is NOT kept in React state nor on
// this socket — each focused terminal attaches to its own dedicated `PtySocket`
// (`lib/ptySocket.ts`).

export interface DuxState {
  // The workspace "spine" from `GET /api/v1/spine`: projects, sessions, and the
  // core-computed sidebar grouping. These three fields used to ride the broadcast
  // `ViewModel`; they now live here, fetched once after auth resolves (alongside
  // the bootstrap document) and re-fetched on a `projects.changed` /
  // `sessions.changed` event or an events-socket reconnect. `null` until the
  // first fetch lands — every consumer falls back to empty lists so nothing
  // crashes in that pre-load window.
  spine: Spine | null
  // The build-static / config-derived document from `GET /api/v1/bootstrap`
  // (providers, macros, palette commands, welcome tips, version, UI flags,
  // global env). Fetched once after auth resolves and re-fetched on a
  // `config.changed` event. `null` until the first fetch lands — every consumer
  // falls back to a sensible default (empty list / true / the scrollback
  // default) so nothing crashes in that pre-load window.
  bootstrap: Bootstrap | null
  // Set to true synchronously when boot runs (at module load). Tests wait on
  // this as a settled signal instead of the old auth.phase guard.
  booted: boolean
  conn: ConnState
  selectedTarget: SelectedTarget | null
  // Derived from `selectedTarget`: the owning session id. Session-scoped UI
  // (breadcrumb, changed files, statusbar) reads this so it keeps working
  // whether an agent or one of its terminals is focused. Kept in `state` (not
  // recomputed per snapshot) so `getSnapshot` stays referentially stable.
  selectedSessionId: string | null
  // Bumped on every reconnect/force-reconnect so the focused TerminalPane
  // remounts and re-subscribes. The reconnect replaces the server-side provider
  // with a new PtyClient; the old PtyClient's byte forwarder is dead, so an
  // already-focused pane (same target id) must re-issue `subscribe` to attach to
  // the new provider. Folded into the pane's React key alongside the target id.
  terminalEpoch: number
  commitTarget: string | null
  commitDraft: string
  deleteTarget: string | null
  // The companion terminal id pending close confirmation, or null. Mirrors the
  // TUI, which ALWAYS confirms terminal deletion (the running process is killed).
  deleteTerminalTarget: string | null
  // The unstaged file pending discard confirmation, or null. The TUI confirms
  // every discard (it's destructive); the web mirrors that.
  discardTarget: DiscardTarget | null
  globalEnvOpen: boolean
  projectSettingsTarget: string | null
  // The agent (session) whose startup-command / project-env editor is open, or
  // null. Both edit the agent's PROJECT (env and startup command are
  // project-scoped in dux — there is no per-agent env), surfaced from the agent
  // menu for quick access (mirroring the TUI's per-agent palette commands). The
  // dialog resolves the owning project from the session id.
  agentStartupCommandTarget: string | null
  agentEnvTarget: string | null
  // The agent (session) whose startup-command log viewer is open, or null. The
  // log files + the displayed file's contents are fetched over REST into the
  // fields below when the viewer opens (mirroring the attach-worktree listing).
  startupLogsTarget: string | null
  startupLogsEntries: StartupLogEntry[]
  startupLogsSelected: StartupLogContent | null
  startupLogsLoading: boolean
  startupLogsError: string | null
  // The project whose read-only info modal is open, or null (closed). Pure
  // presentation of existing ViewModel data — no wire command, no git read.
  projectInfoTarget: string | null
  addProjectOpen: boolean
  browsePath: string
  browseEntries: DirEntryView[]
  browseLoading: boolean
  // Branch pre-flight for the add-project flow, mirroring the TUI's
  // `ConfirmNonDefaultBranch` prompt. When the user selects a git repo the
  // dialog fires `inspectProjectPath`; the reply lands here keyed by `path` so a
  // stale reply for a previously-selected repo is ignored. `loading` drives the
  // dialog's spinner; `warning` null (with a resolved `path`) means the repo is
  // on its default branch — no warning step. `null` overall means no inspection
  // is pending or resolved (nothing selected).
  projectPathInspection: {
    path: string
    currentBranch: string | null
    warning: BranchWarningView | null
    error: string | null
    loading: boolean
  } | null
  removeProjectTarget: string | null
  // The project pending a default-branch checkout confirmation, or null. The
  // checkout moves the source checkout's HEAD, so the web confirms first (the
  // TUI runs it straight from a deliberate palette/keybinding action).
  checkoutDefaultBranchTarget: string | null
  // The project whose managed worktrees are being browsed for adoption, or null
  // (closed). The dialog requests the listing on open; `attachWorktreeEntries`
  // holds the server's classification and `attachWorktreeLoading` drives the
  // spinner until the `project_worktrees` reply lands. Mirrors the TUI's
  // `new-agent-from-worktree` picker.
  attachWorktreeTarget: string | null
  attachWorktreeEntries: ProjectWorktreeEntryView[]
  attachWorktreeLoading: boolean
  // The name-input dialog target: a fresh agent in a project, a fork of an
  // existing session, or null (closed). One dialog component switches on `kind`.
  createAgentTarget: CreateAgentTarget | null
  // The session pending a rename, or null. The dialog pre-fills the current
  // title (or empty, so the placeholder shows the branch name).
  renameTarget: string | null
  renameDraft: string
  // The session pending a provider swap, or null. The dialog pre-selects the
  // session's current provider; the swap takes effect on the next launch
  // (mirroring the TUI's `change-agent-provider`, which never kills a running
  // agent — it changes the provider for the next reconnect).
  changeProviderTarget: string | null
  // New-agent dialog state lives in the store (like `commitDraft`) so the input
  // is fully store-controlled: the server's generated-name reply fills it via an
  // event-driven callback, never a set-state-in-effect. Mirrors the TUI prompt.
  //   - `createAgentDraft`: the sanitized branch-name input.
  //   - `createAgentRandomize`: the "Use randomized pet name" checkbox.
  //   - `createAgentGeneratedName`: the last name the server generated, so an
  //     uncheck clears the input ONLY when it still equals that name (exact TUI
  //     semantics); null once the user edits away from it or no name is pending.
  createAgentDraft: string
  createAgentRandomize: boolean
  createAgentGeneratedName: string | null
  //   - `createAgentPrInput`: the raw PR reference (URL, `#123`, or `123`) for
  //     the "From PR" mode. Free text (NOT agent-name-sanitized); the server
  //     parses it against the project's GitHub remote. Empty in the other modes.
  createAgentPrInput: string
  //   - `createAgentNamePending`: a generate-name request is in flight. Drives
  //     the dialog's spinner and disables the input so a late reply can never
  //     clobber text the user typed in the meantime. Explicit rather than
  //     inferred from an empty draft, so manually clearing the input doesn't
  //     fake a phantom "generating" state.
  createAgentNamePending: boolean
  paletteOpen: boolean
  // The macro-editor dialog. `macrosDialogOpen` gates the modal; `macrosDraft`
  // is the working copy of the whole macro list the user edits before saving
  // (the save is wholesale — `update_macros` replaces the entire `[macros]`
  // map, mirroring the TUI editor). Seeded from `bootstrap.macros` on open so
  // there is no set-state-in-effect. Empty draft when closed.
  macrosDialogOpen: boolean
  macrosDraft: MacroView[]
  // Which screen the mobile shell is showing. Always "home" on desktop, which
  // ignores it. Only the mobile UI advances it past "home".
  mobileScreen: MobileScreen
  // Optimistic drag-and-drop ordering overlays (see `applyPendingOrders`). Each
  // is set the moment a drag ends and cleared once the server's next spine
  // confirms the new order (or an error status arrives). Null when no reorder is
  // in flight, which is the overwhelmingly common case.
  pendingSessionOrder: PendingSessionOrder | null
  pendingProjectOrder: string[] | null
  // While an agent-create THIS client initiated is in flight, holds the session
  // ids that already existed when we submitted, plus the project the new agent
  // will land in. Agent creation is an async server job whose only completion
  // signal is a `sessions.changed` event + spine refetch (no per-client reply, no request/echo
  // correlation), so we recognize "our" new agent as the session id that appears
  // in `projectId` and wasn't in `knownIds`, then focus it — mirroring the TUI,
  // which jumps selection to a freshly created agent when its launch completes.
  // Only the client that armed this reacts, so other connected clients aren't
  // yanked off whatever they're viewing. Null when no create is awaiting focus.
  // See `armCreateFocus` and `focusNewlyCreatedSession`.
  // `armedAt` (epoch ms) bounds the token's lifetime: a create that never lands
  // (the dispatch failed silently server-side, or the agent took absurdly long)
  // would otherwise leave the token armed forever, ready to mis-focus the next
  // unrelated session that happens to appear in `projectId`. See
  // `CREATE_FOCUS_TTL_MS` and `focusNewlyCreatedSession`.
  pendingCreateFocus: {
    knownIds: string[]
    projectId: string
    armedAt: number
  } | null
  sidebarWidth: string
  // Optimistic override for the Changes pane's visibility (desktop). `null`
  // follows the persisted config (`bootstrap.show_changes_pane`); the palette and
  // the Changes actions menu set an explicit bool for instant feedback. The
  // toggle persists to config via the server; this clears once the broadcast
  // confirms (or on command error / disconnect, which roll it back).
  changesPaneOverride: boolean | null
  // The session whose code-editor overlay is open, the file to auto-open on
  // launch (null = none preselected), and the view it opens in: "file" (editable
  // Monaco buffer) or "diff" (read-only Monaco DiffEditor, HEAD vs working copy).
  // The editor always operates on the SELECTED session, so opening it selects
  // that session first and reuses the existing changed-files broadcast for its
  // file list. Null = overlay closed.
  editorTarget: {
    sessionId: string
    initialPath: string | null
    initialMode: EditorViewMode
  } | null
  // Changed-files state for the selected session (see `ChangesSlice`). The single
  // source for changed-files data — replaces the global `viewModel.changed_files`
  // broadcast, which a second client could clobber.
  changes: ChangesSlice
}

// Which view the code editor opens in (and toggles between): the editable Monaco
// buffer, or the read-only Monaco diff (HEAD vs working copy). Opening a changed
// file defaults to "diff"; the file tree / edit actions default to "file".
export type EditorViewMode = "file" | "diff"

// The expanded sidebar width is drag-resizable and persisted across reloads.
// 18rem gives agent names breathing room next to the PR/status badges; a
// previously persisted width still wins.
const SIDEBAR_WIDTH_KEY = "dux:sidebar-width"
const DEFAULT_SIDEBAR_WIDTH = "18rem"

function loadSidebarWidth(): string {
  return localStorage.getItem(SIDEBAR_WIDTH_KEY) || DEFAULT_SIDEBAR_WIDTH
}

// One-time cleanup: the diff line-number toggle (and its persisted preference)
// went away when the web diff moved to Monaco, which manages its own gutters.
// Drop the orphaned key so it can't linger or be misread by a future feature.
localStorage.removeItem("dux:show-diff-line-numbers")

// The `/ws/events` topic for one session's changed files.
function changesTopic(sessionId: string): string {
  return `session:${sessionId}:changes`
}

// A cleared changed-files slice (nothing selected, no data).
function emptyChanges(): ChangesSlice {
  return {
    sessionId: null,
    phase: "idle",
    rev: 0,
    staged: [],
    unstaged: [],
    error: null,
  }
}

// A fresh slice for `sessionId` entering its loading window. `rev: 0` so the
// first successful response (rev >= 1 from the server) always applies.
function loadingChanges(sessionId: string): ChangesSlice {
  return {
    sessionId,
    phase: "loading",
    rev: 0,
    staged: [],
    unstaged: [],
    error: null,
  }
}

let state: DuxState = {
  spine: null,
  bootstrap: null,
  booted: false,
  conn: "connecting",
  selectedTarget: null,
  selectedSessionId: null,
  terminalEpoch: 0,
  commitTarget: null,
  commitDraft: "",
  deleteTarget: null,
  deleteTerminalTarget: null,
  discardTarget: null,
  globalEnvOpen: false,
  projectSettingsTarget: null,
  agentStartupCommandTarget: null,
  agentEnvTarget: null,
  startupLogsTarget: null,
  startupLogsEntries: [],
  startupLogsSelected: null,
  startupLogsLoading: false,
  startupLogsError: null,
  projectInfoTarget: null,
  addProjectOpen: false,
  browsePath: "",
  browseEntries: [],
  browseLoading: false,
  projectPathInspection: null,
  removeProjectTarget: null,
  checkoutDefaultBranchTarget: null,
  attachWorktreeTarget: null,
  attachWorktreeEntries: [],
  attachWorktreeLoading: false,
  createAgentTarget: null,
  renameTarget: null,
  renameDraft: "",
  changeProviderTarget: null,
  createAgentDraft: "",
  createAgentRandomize: false,
  createAgentGeneratedName: null,
  createAgentNamePending: false,
  createAgentPrInput: "",
  paletteOpen: false,
  macrosDialogOpen: false,
  macrosDraft: [],
  mobileScreen: "home",
  pendingSessionOrder: null,
  pendingProjectOrder: null,
  pendingCreateFocus: null,
  sidebarWidth: loadSidebarWidth(),
  changesPaneOverride: null,
  editorTarget: null,
  changes: emptyChanges(),
}

const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

function setState(patch: Partial<DuxState>): void {
  state = { ...state, ...patch }
  emit()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

// The external-store snapshot accessor `useSyncExternalStore` consumes. Exported
// so unit tests can read the live state after dispatching an action (there is no
// React harness in this test setup); production code reads it only via `useDux`.
export function getSnapshot(): DuxState {
  return state
}

// Derive the WebSocket scheme from the page protocol so an HTTPS deployment uses
// `wss://` (a hardcoded `ws://` would be blocked as mixed content under HTTPS).
const wsScheme = location.protocol === "https:" ? "wss:" : "ws:"

// The single JSON socket for the whole app (`/ws/events`), separate from the
// per-PTY byte sockets (`lib/ptySocket.ts`). Since the Phase 6 cutover it carries
// EVERYTHING the retired `/ws`/`DuxSocket` used to: resource-change events
// (changed files, spine, config) AND the control frames
// (`connected` id, `status`/`status_cleared` toasts). It also owns the
// connection-state UX (the status-bar indicator). Exported so tests can drive
// its callbacks / inspect its interest set; connected on boot.
export const eventsSocket = new EventsSocket(
  `${wsScheme}//${location.host}/ws/events`,
)

// App-wide coarse topics, subscribed once at module load. They are added to the
// interest set immediately (sent on the first open, re-sent on every reconnect).
// Phase 1 has no GET tied to these — they exist so later phases can refresh
// projects/sessions/config off the same channel without a new subscribe site.
eventsSocket.subscribe(["sessions", "projects", "config"])

// A `session.changes` event invalidates one session's changed files. Refetch
// when it is the selected session AND (the slice is in error — always re-fetch
// to self-heal, since the error path has no usable rev) OR the event's rev is at
// least the applied rev. Lag catch-up arrives as the same event, so this one
// handler covers it.
eventsSocket.onEvent = (ev: EventsServerMessage) => {
  // The per-connection id, delivered as the FIRST `/ws/events` frame (and re-sent
  // on every reconnect). Record it so the REST clients can stamp it as
  // `X-Connection-Id` and the server scopes their status toasts back to us.
  if (ev.event === "connected") {
    if (typeof ev.id === "string") setConnectionId(ev.id)
    return
  }
  // Engine status toasts, migrated off the retired `/ws`. The server already
  // scope-filtered, so the `scope` field is ignored client-side. An error-toned
  // async status also voids any in-flight create-focus (the create likely failed)
  // and unwinds any optimistic reorder overlay — mirroring the old `onStatus`.
  if (ev.event === "status") {
    if (ev.tone === "error") setState({ ...clearPendingClientIntent() })
    showStatusToast(ev.key, ev.tone ?? "info", ev.message ?? "")
    return
  }
  // Dismiss the toast whose id matches the cleared key (anonymous slot when null).
  if (ev.event === "status_cleared") {
    toast.dismiss(ev.key ?? ANON_TOAST_ID)
    return
  }
  // A `config.changed` event invalidates the bootstrap document (the server's
  // config was edited/reloaded). Re-GET it so providers, macros, UI flags, etc.
  // reflect the new config without a reconnect. The `config` coarse topic is
  // subscribed at module load, so this fires for every client.
  if (ev.event === "config.changed") {
    loadBootstrap()
    return
  }
  // A `projects.changed` / `sessions.changed` event invalidates the workspace
  // spine (a project/session was added, removed, reordered, renamed, changed
  // status, etc.). Re-GET it so the sidebar, session lists, and selection logic
  // reflect the new state. The `projects`/`sessions` coarse topics are subscribed
  // at module load, so this fires for every client. The applied spine drives the
  // focus/prune/reorder reconciliation (see `applySpine`).
  if (ev.event === "projects.changed" || ev.event === "sessions.changed") {
    loadSpine()
    return
  }
  // A `pty.owner` event means a connection claimed (took over, or first-claimed an
  // unowned) PTY's sizing+input. Fan it out to the mounted terminal view for that
  // pty id along with the claimer's connection id (`owner`); the view compares that
  // against its own PTY-socket connection id to decide definitively whether it is
  // the owner (stays interactive) or has been taken over (read-only placeholder).
  // The id is the pty id (session id for an agent, terminal id for a companion).
  // Delivered on the coarse `sessions` topic, subscribed at module load.
  if (ev.event === "pty.owner") {
    // Pass the ownership epoch so the fan-out can ignore an out-of-order (older)
    // handover and converge on the latest claim regardless of arrival order.
    if (typeof ev.id === "string") notifyPtyOwner(ev.id, ev.owner, ev.epoch)
    return
  }
  if (ev.event !== "session.changes") return
  const id = ev.id
  if (id === undefined || id !== state.selectedSessionId) return
  // A missing `rev` (the server's Lagged catch-up for a cold session carries
  // none) is a force-refetch: we can't compare it, so we must NOT let
  // `undefined >= rev` short-circuit to false and skip the refetch.
  const rev = ev.rev
  if (
    state.changes.phase === "error" ||
    rev === undefined ||
    rev >= state.changes.rev
  ) {
    loadChanges(id)
  }
}

// The `boot()` driver kicks off the very first bootstrap+spine load alongside
// `eventsSocket.connect()`, so the first `onOpen` that follows must NOT re-fetch
// them or it would duplicate that initial load. The driver sets this flag right
// before connecting; the first `onOpen` consumes it. Every
// later (RE-connect) open leaves it false and so always retries — crucially, even
// when the FIRST load FAILED: keying the retry off `state.bootstrap !== null`
// (the old guard) stranded a failed first fetch as null forever, so every
// reconnect skipped it and the app stayed empty with no recovery path.
let skipNextEventsOnOpenLoad = false

// After a (re)connect the socket has re-sent the whole interest set; re-fetch so
// anything missed while disconnected is recovered (an event that arrived during
// the outage is gone otherwise). The `config` coarse topic is always subscribed,
// so refetch the bootstrap document too — a `config.changed` missed during the
// outage would otherwise leave stale providers/macros/UI flags until the next
// config edit. The selected session's changes are also recovered when one is set.
eventsSocket.onOpen = () => {
  if (skipNextEventsOnOpenLoad) {
    // First open after a boot/login load: skip the duplicate fetch this once.
    skipNextEventsOnOpenLoad = false
  } else {
    // A reconnect (or an open the driver did not pre-load for): re-fetch both so
    // anything missed during the outage — or a load that failed on first boot —
    // recovers. Concurrent loads are safe: spine is seq-guarded and bootstrap
    // apply is idempotent.
    loadBootstrap()
    loadSpine()
    // The server's ownership epoch counter restarts at zero if the server itself
    // restarted during the outage; clear our per-pty high-water marks so a fresh
    // post-restart `pty.owner` is not wrongly ignored as stale. A reconnect is the
    // only path a restarted server's epochs reach us, and there is no `pty.owner`
    // replay, so this can never drop a still-relevant in-flight handover.
    resetPtyOwnerEpochs()
  }
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  loadChanges(id)
}

// Move the changed-files subscription from one session to another. A null side
// means "no session" (clear/select-nothing). A no-op when unchanged.
function switchChangesSubscription(
  prev: string | null,
  next: string | null,
): void {
  if (prev === next) return
  if (prev !== null) eventsSocket.unsubscribe([changesTopic(prev)])
  if (next !== null) eventsSocket.subscribe([changesTopic(next)])
}

// Fire a changed-files fetch for `sessionId` and route the outcome through the
// guarded apply/error handlers. Errors are caught here so a failed fetch can
// never surface as an unhandled rejection.
function loadChanges(sessionId: string): void {
  fetchChanges(sessionId)
    .then((resp) => applyChangesResponse(sessionId, resp))
    .catch((err) => applyChangesError(sessionId, err))
}

// Apply a fetch response, dropping it when it lost a race. Two guards:
//   1. the requested session must still be selected AND own the slice (a fast
//      session switch already moved on); and
//   2. the response `rev` must be >= the applied `rev` (an older, out-of-order
//      response must not overwrite newer data).
function applyChangesResponse(
  sessionId: string,
  resp: SessionChangesResponse,
): void {
  if (state.selectedSessionId !== sessionId) return
  if (state.changes.sessionId !== sessionId) return
  if (resp.rev < state.changes.rev) return
  setState({
    changes: {
      sessionId,
      phase: "loaded",
      rev: resp.rev,
      staged: resp.staged,
      unstaged: resp.unstaged,
      error: null,
    },
  })
}

// Apply a failed fetch. A 404 means the session is gone — clear the slice (the
// next spine's `pruneSelectionIfGone` clears the selection). Anything else
// (409 git lock, 5xx, network) lands in `error` so the pane shows a Refresh
// affordance; the poller's eventual recovery event self-heals it. Same staleness
// guards as the success path so a late failure can't clobber a newer state.
function applyChangesError(sessionId: string, err: unknown): void {
  if (state.selectedSessionId !== sessionId) return
  if (state.changes.sessionId !== sessionId) return
  if (err instanceof ChangesFetchError && err.status === 404) {
    setState({ changes: emptyChanges() })
    return
  }
  // Only the fetch that opened the current loading window may flip the slice to
  // error. A late failure that lost the race to a successful concurrent fetch
  // (e.g. a slow 409 arriving after a newer 200 already loaded the pane) must
  // not turn a loaded pane into an error pane. The next `session.changes` event
  // still self-heals an error state regardless.
  if (state.changes.phase !== "loading") return
  const message =
    err instanceof Error ? err.message : "Could not load changed files."
  setState({
    changes: { ...state.changes, sessionId, phase: "error", error: message },
  })
}

// Re-fetch the selected session's changes (the changes pane's Refresh button).
// No-op when nothing is selected.
export function refreshChanges(): void {
  const id = state.selectedSessionId
  if (id === null) return
  setState({ changes: loadingChanges(id) })
  loadChanges(id)
}

// Fetch the bootstrap document and fold it into state. Errors are swallowed: on
// first boot the slice stays `null` (consumers fall back to defaults) and a
// later `config.changed` event or a reconnect retries; on a refetch the last
// good bootstrap is kept rather than blanking the UI. Never surfaces as an
// unhandled rejection.
function loadBootstrap(): void {
  fetchBootstrap()
    .then((b) => applyBootstrap(b))
    .catch((err) => {
      // Keep the previous bootstrap (null on first boot); a config.changed event
      // or reconnect will retry. Warn so a persistently-failing fetch (e.g. a
      // first boot that stays empty) is visible in the console rather than silent.
      console.warn("[dux] bootstrap fetch failed; will retry on reconnect", err)
    })
}

// Apply a freshly fetched bootstrap. Also reconciles the optimistic Changes-pane
// override the same way the broadcast ViewModel used to: the toggle persists to
// config, the server emits `config.changed`, the refetched bootstrap carries the
// confirmed `show_changes_pane`, and the override is dropped once it matches so
// config becomes the single source of truth across every client.
function applyBootstrap(b: Bootstrap): void {
  setState({
    bootstrap: b,
    changesPaneOverride:
      state.changesPaneOverride !== null &&
      state.changesPaneOverride === b.show_changes_pane
        ? null
        : state.changesPaneOverride,
  })
  // Reflect the configured instance name in the browser tab. Guarded because the
  // store also runs under the Node test environment, where `document` is absent
  // unless a test stubs it. Runs on first load and on every config.changed
  // refetch, so a live rename updates the tab without a reload.
  if (typeof document !== "undefined") {
    document.title = resolveInstanceTitle(b.title)
  }
  // Swap the favicon to the configured one (bundled logo, a recoloured dux-logo
  // outline, or a custom URL). Self-guards on the DOM, so it is a no-op under the
  // store's Node test environment. Runs on first load and every config.changed.
  applyFavicon(b.favicon)
}

// Monotonic sequence for spine loads. Two rapid `sessions.changed`/
// `projects.changed` events fire concurrent `fetchSpine()`s; without a guard an
// older response resolving last would overwrite a newer spine (observable as a
// focus-then-prune-clear flicker on agent create). Each `loadSpine` captures the
// seq it bumped to; `applySpine` discards a result once a newer load has started.
// Mirrors the `applyChangesResponse` rev-guard, but with a client-side counter
// (the spine read has no server rev).
let loadSpineSeq = 0

// Fetch the workspace spine and fold it into state. Errors are swallowed: on
// first boot the slice stays `null` (consumers fall back to empty lists) and a
// later `projects.changed`/`sessions.changed` event or a reconnect retries; on a
// refetch the last good spine is kept rather than blanking the sidebar. Never
// surfaces as an unhandled rejection.
function loadSpine(): void {
  const seq = ++loadSpineSeq
  fetchSpine()
    .then((s) => applySpine(s, seq))
    .catch((err) => {
      // Keep the previous spine (null on first boot); an event or reconnect will
      // retry. Warn so a persistently-failing fetch (e.g. a first boot that stays
      // empty) is visible in the console rather than silent.
      console.warn("[dux] spine fetch failed; will retry on reconnect", err)
    })
}

// Apply a freshly fetched spine. This is the single place the projects/sessions/
// sidebar data lands, and it drives the same client-view reconciliation the
// broadcast ViewModel used to:
//   - retire the optimistic reorder overlays once the server's order matches;
//   - auto-focus an agent THIS client just created, the instant it appears;
//   - prune the selection when its target session/terminal has vanished.
// Order mirrors the legacy `onViewModel`: set the slice (with reconciled overlays)
// first, then focus (which only ever selects a session present in the spine, so
// the prune below leaves it alone), then prune.
//
// `seq` is the `loadSpineSeq` value the originating `loadSpine` captured; discard
// this (now-stale) result if a newer load has since started, so a slow older
// response can never overwrite a fresher spine (and re-run focus/prune against
// outdated data).
function applySpine(spine: Spine, seq: number): void {
  if (seq < loadSpineSeq) return
  setState({
    spine,
    pendingSessionOrder: reconcilePendingSessionOrder(spine, state.pendingSessionOrder),
    pendingProjectOrder: reconcilePendingProjectOrder(spine, state.pendingProjectOrder),
  })
  // Restore a boot-time deep-link before focus/prune: it selects only a session
  // present in this spine (so prune leaves it alone), and it is a one-shot that
  // self-clears, so it never fights a create-focus or a later refetch.
  restoreDeepLink(spine)
  focusNewlyCreatedSession(spine)
  pruneSelectionIfGone(spine)
}

// The per-connection id now arrives as the `connected` event on `/ws/events`
// (see `eventsSocket.onEvent` above), which calls `setConnectionId`. The REST
// clients stamp it as `X-Connection-Id` so the server scopes their toasts back
// to this client.

// The broadcast ViewModel now carries ONLY `changed_files` (a residual frame);
// projects/sessions/sidebar moved to `GET /api/v1/spine` and the changed-files
// data is owned by the `changes` slice over REST. Nothing reads the residual
// frame anymore, so we deliberately do NOT install an `onViewModel` handler:
// storing it on every frame only triggered spurious re-renders. The
// focus/prune/reorder reconciliation runs on the spine apply path (`applySpine`),
// and changed files flow through the `changes` slice. The frame is removed at
// cutover (Phase 6); until then it is simply ignored (the socket default no-op).

// Drop the pending session-order overlay once the incoming spine's session
// order for that project already equals the overlay; otherwise keep it.
function reconcilePendingSessionOrder(
  spine: Spine,
  pending: PendingSessionOrder | null,
): PendingSessionOrder | null {
  if (!pending) return null
  const serverIds = spine.sessions
    .filter((s) => s.project_id === pending.projectId)
    .map((s) => s.id)
  return ordersMatch(serverIds, pending.ids) ? null : pending
}

// Drop the pending project-order overlay once the incoming spine's project
// order already equals the overlay; otherwise keep it.
function reconcilePendingProjectOrder(
  spine: Spine,
  pending: string[] | null,
): string[] | null {
  if (!pending) return null
  const serverIds = spine.projects.map((p) => p.id)
  return ordersMatch(serverIds, pending) ? null : pending
}

// Clear the selection when its target no longer exists in the latest spine.
// Agents persist after exiting (their session stays, marked detached), so they
// only vanish on deletion; terminals are removed outright when their PTY exits.
function pruneSelectionIfGone(spine: Spine): void {
  const target = state.selectedTarget
  if (!target) return
  const stillExists =
    target.kind === "agent"
      ? spine.sessions.some((s) => s.id === target.sessionId)
      : spine.sessions.some((s) =>
          s.terminals.some((t) => t.id === target.terminalId),
        )
  if (!stillExists) {
    // `selectSession(null)` clears the target and, on mobile, unwinds the spoke
    // so the back stack matches the screen (see `unwindMobileSpoke`). This is
    // the other out-of-band clear path: a terminal whose PTY exited is dropped
    // from the ViewModel while the user may be sitting in its spoke.
    selectSession(null)
  }
}

// Snapshot the session ids that exist right now and arm auto-focus for an agent
// THIS client is creating, so the next spine carrying a new id in `projectId`
// is recognized as our new agent and focused (see `focusNewlyCreatedSession`).
// Call this immediately before dispatching an agent-create command; it is wired
// into `submitNameDialog` (new/fork/from-PR) and `attachWorktree`. Re-arming
// overwrites any prior pending focus, so a fresh create supersedes an earlier
// one whose agent never arrived. Always pass the project the new agent will land
// in — the match is project-scoped, so a caller that cannot resolve the project
// must skip arming rather than pass a placeholder.
// How long an armed create-focus token stays live before it self-expires. Set
// comfortably above the longest server-side create window (the from-PR create
// awaits up to 60s — see `FROM_PR_CREATE_AWAIT_TIMEOUT`) so a legitimate slow
// create still auto-focuses, but bounded so a create that never lands cannot keep
// a stale token armed to grab a later, unrelated session.
const CREATE_FOCUS_TTL_MS = 90_000

function armCreateFocus(projectId: string): void {
  const knownIds = (state.spine?.sessions ?? []).map((s) => s.id)
  setState({ pendingCreateFocus: { knownIds, projectId, armedAt: Date.now() } })
}

// Focus the agent THIS client just created, the instant it shows up. With a
// pending-focus token armed (`armCreateFocus`), scan the incoming spine for a
// session that wasn't known at submit time and lives in the expected project,
// select it (which points the changed-files watch at it; the focused TerminalPane
// subscribes its PTY on mount), and disarm. No-op — and cheap — when nothing is
// pending, the overwhelmingly common case. Other clients never armed a token, so
// they don't react: focus moves only on the client that initiated the create.
function focusNewlyCreatedSession(spine: Spine): void {
  const pending = state.pendingCreateFocus
  if (!pending) return
  // Expire a stale token rather than letting it focus an unrelated session that
  // appears long after the create it was armed for (a silently-failed create, or
  // one that never completed). Disarm and bail.
  if (Date.now() - pending.armedAt > CREATE_FOCUS_TTL_MS) {
    setState({ pendingCreateFocus: null })
    return
  }
  const known = new Set(pending.knownIds)
  const created = spine.sessions.find(
    (s) => !known.has(s.id) && s.project_id === pending.projectId,
  )
  if (!created) return
  // Consume the token before selecting so a later spine can't re-fire.
  setState({ pendingCreateFocus: null })
  selectSession(created.id)
}

eventsSocket.onConn = (conn) => {
  // A connection break invalidates any in-flight optimistic reorder: the
  // command (or its rejection) may have been lost, and after the reconnect
  // nothing would ever reconcile a non-matching overlay — leaving the UI
  // showing an order the server never persisted. Snap back to authoritative.
  // The same break also voids any pending create-focus: its `knownIds` snapshot
  // predates the disconnect, so diffing it against the post-reconnect ViewModel
  // could mis-identify an unrelated session as "ours". Drop it and let the user
  // pick up the new agent from the sidebar.
  const patch =
    conn === "closed" || conn === "failed" ? clearPendingClientIntent() : {}
  setState({ conn, ...patch })
  // Clear the per-connection id on a drop. It belongs to the now-dead socket; a
  // REST action fired during the reconnect window must NOT stamp it as
  // `X-Connection-Id`, or the server would scope that action's status toasts to a
  // connection that no longer exists and the user would never see them. A null id
  // falls back to scope `All` (broadcast) — visible to this client once it
  // reconnects, the safe default. The next `connected` frame re-issues a fresh id.
  if (conn === "closed" || conn === "failed") setConnectionId(null)
  // Changed files no longer ride this socket: the `/ws/events` channel owns the
  // per-session subscription and re-establishes it on its own reconnect (see
  // `eventsSocket.onOpen`, which also refetches). There is nothing to re-arm here.
}

// Since Phase 6 there is no `command_result`/`error` frame: every action is a
// REST verb whose failure rejects its promise (the caller toasts it and rolls
// back optimistic state), and every keyed busy/success/clear arrives as a
// `status`/`status_cleared` event over `/ws/events` (see `eventsSocket.onEvent`).

// Reset both optimistic order overlays. Returned as a patch so callers can fold
// it into a single `setState`. Used on every error path so a rejected reorder
// snaps the UI back to the server's authoritative order.
function clearPendingOrders(): Partial<DuxState> {
  return { pendingSessionOrder: null, pendingProjectOrder: null }
}

// Clear every transient, optimistic client intent at once: the reorder overlays,
// any pending create-focus, AND the Changes-pane visibility override. Used on the
// failure/teardown paths (command error, async error status, socket disconnect)
// where an in-flight create can no longer be trusted to resolve — a surviving
// `pendingCreateFocus` snapshot would otherwise mis-identify a later, unrelated
// session as the one we created, and a surviving Changes-pane override would
// strand the pane in the toggled state until reload. NOT folded into
// `clearPendingOrders` because user actions like sorting also clear the order
// overlays but must NOT cancel an in-flight create-focus.
function clearPendingClientIntent(): Partial<DuxState> {
  return {
    ...clearPendingOrders(),
    pendingCreateFocus: null,
    changesPaneOverride: null,
  }
}

// Stable sonner id for the anonymous (no-key) status slot. Sonner otherwise
// assigns a random id on each call, making anonymous clears a no-op and every
// anonymous update a new transient toast instead of an in-place update.
const ANON_TOAST_ID = "dux-anon-status"

// Route a keyed (or anonymous) engine status to both the status bar and a
// sonner toast. The key acts as the sonner id so updates re-render in place
// (busy → success swaps the spinner without a new toast) and clears can dismiss
// by id. Busy must not auto-dismiss before its final state arrives, so its
// duration is Infinity.
function showStatusToast(
  key: string | null | undefined,
  tone: string,
  message: string,
): void {
  if (!message) return
  const id = key ?? ANON_TOAST_ID // no key → stable anonymous-slot id
  // Info/success toasts auto-clear after the configured window
  // (`config.ui.status_clear_seconds`, default 6); 0 disables auto-clear so they
  // stay sticky like a warning/error. Busy/warning/error never auto-dismiss
  // (their final state replaces them). A missing bootstrap (pre-load) falls back
  // to the 6s default.
  const secs = state.bootstrap?.status_clear_seconds ?? 6
  const infoDuration = secs === 0 ? Infinity : secs * 1000
  const duration = tone === "info" ? infoDuration : Infinity
  const opts = { id, duration }
  if (tone === "error") toast.error(message, opts)
  else if (tone === "warning") toast.warning(message, opts)
  else if (tone === "busy") toast.loading(message, opts)
  else toast.success(message, opts) // info/success
}

// Boot: connect the events socket and fetch the initial workspace data. No
// /api/me round-trip is needed -- the server is a trusted-local tool with no
// login gate. Setting booted synchronously lets tests use it as a settled signal.
function boot(): void {
  setState({ booted: true })
  // This driver owns the initial load, so the first onOpen must not duplicate it
  // (every later reconnect still retries -- see the flag's docs).
  skipNextEventsOnOpenLoad = true
  eventsSocket.connect()
  loadBootstrap()
  loadSpine()
}
boot()

// Hardware/browser Back for the mobile shell. Registered ONCE at module scope
// (never in a React effect) so it survives re-renders and shell switches. The
// browser has already popped its own entry by the time this fires, so we only
// mirror that into our screen state. The target is derived from our own state
// machine — changes unwinds to the terminal when a target is still focused
// (else home), terminal unwinds to home — not from event.state contents, which
// keeps it resilient to history entries we didn't author. When mobileScreen is
// already "home" there is no spoke to unwind, so we no-op; this is also why
// desktop (which never advances past "home") is unaffected.
window.addEventListener("popstate", () => {
  const current = state.mobileScreen
  if (current === "home") return
  if (current === "changes") {
    setState({ mobileScreen: state.selectedTarget ? "terminal" : "home" })
  } else {
    setState({ mobileScreen: "home" })
  }
})

// INVARIANT: the number of history entries we've pushed equals the spoke depth
// implied by `mobileScreen` (home = 0, terminal = 1, changes = 2). `mobileNavigate`
// pushes on the way in; the popstate listener above pops on the way out. When the
// focused target is cleared OUT-OF-BAND (an agent exits, or a terminal is pruned
// from the ViewModel) the screen would otherwise fall back to home content while
// our pushed entries linger, leaving Back as a stale no-op (terminal) or a
// double-back (changes). This collapses the whole spoke back to home in one
// `history.go`, which fires a SINGLE popstate at the destination; the listener
// above then derives `mobileScreen: "home"` (selectedTarget is null by the time
// it runs because callers clear it first), restoring the invariant.
function unwindMobileSpoke(): void {
  if (state.mobileScreen === "terminal") {
    history.go(-1)
  } else if (state.mobileScreen === "changes") {
    history.go(-2)
  }
  // "home": no spoke entries to unwind. Desktop never advances past "home", so
  // it never reaches this branch with entries to pop — desktop is untouched.
}

export function useDux(): DuxState {
  return useSyncExternalStore(subscribe, getSnapshot)
}

// --- Deep-linking (a tiny hash router) ------------------------------------
//
// The selected target is mirrored into `location.hash` so a tab can be bookmarked
// /shared/reloaded back to the same agent (and, when one is focused, terminal):
//   #/agent/<sessionId>
//   #/agent/<sessionId>/terminal/<terminalId>
// Session ids are stable (a reload restores the agent); terminal ids are
// ephemeral (a reload that finds the session but not the terminal falls back to
// the agent; one that finds neither ignores the link). The hash is written with
// `history.replaceState` so it never adds a back-stack entry (that would fight
// the mobile spoke/back-button model, which uses `pushState`/`go`).

// Parse a deep-link hash into a target, or null when it is absent/malformed.
function parseSelectionHash(hash: string): SelectedTarget | null {
  const m = hash.match(/^#\/agent\/([^/]+)(?:\/terminal\/([^/]+))?$/)
  if (!m) return null
  // `decodeURIComponent` throws a URIError on malformed percent-encoding (e.g.
  // `#/agent/%ZZ`). This runs at module init, so an unguarded throw would blank
  // the whole app. Treat any decode failure as no/invalid deep link.
  try {
    const sessionId = decodeURIComponent(m[1])
    if (!sessionId) return null
    if (m[2]) {
      const terminalId = decodeURIComponent(m[2])
      if (!terminalId) return null
      return { kind: "terminal", terminalId, sessionId }
    }
    return { kind: "agent", sessionId }
  } catch {
    return null
  }
}

// The hash for a target (or the bare path when nothing is selected).
function selectionHash(target: SelectedTarget | null): string {
  if (!target) return ""
  const base = `#/agent/${encodeURIComponent(target.sessionId)}`
  return target.kind === "terminal"
    ? `${base}/terminal/${encodeURIComponent(target.terminalId)}`
    : base
}

// Mirror the current selection into the URL hash without growing the back stack.
// Defensive: in non-browser test environments `history.replaceState` / a real
// `location` may be absent, so this no-ops there.
function writeSelectionHash(): void {
  if (typeof history === "undefined" || typeof history.replaceState !== "function") {
    return
  }
  const next = selectionHash(state.selectedTarget)
  const current = typeof location !== "undefined" ? location.hash ?? "" : ""
  if (current === next) return
  // An empty target hash collapses to the bare path so the URL doesn't keep a
  // dangling "#"; otherwise replace just the hash, preserving path + query.
  const base =
    typeof location !== "undefined"
      ? (location.pathname ?? "") + (location.search ?? "")
      : ""
  history.replaceState(history.state, "", next === "" ? base : next)
}

// The deep-link parsed from the URL at module load, restored once the first spine
// lands (a target can't be resolved until the session list exists). One-shot:
// consumed (and cleared) on the first `applySpine` so later spine refetches don't
// re-yank a user who has since navigated away.
let pendingDeepLink: SelectedTarget | null =
  typeof location !== "undefined"
    ? parseSelectionHash(location.hash ?? "")
    : null

// Restore the boot-time deep-link against the first spine. Resolve the session in
// the spine; restore the terminal when it still exists, else fall back to the
// session; ignore the link entirely when the session is gone.
function restoreDeepLink(spine: Spine): void {
  const link = pendingDeepLink
  if (!link) return
  pendingDeepLink = null // one-shot, whatever the outcome
  const session = spine.sessions.find((s) => s.id === link.sessionId)
  if (!session) return // session id gone — ignore the link
  if (link.kind === "terminal") {
    const stillThere = session.terminals.some((t) => t.id === link.terminalId)
    if (stillThere) {
      selectTerminal(link.terminalId, link.sessionId)
      return
    }
    // Terminal id gone — fall back to the owning agent.
  }
  selectSession(link.sessionId)
}

// Select an agent session as the streamed target. Signature kept stable so
// existing callers continue to work unchanged.
export function selectSession(id: string | null): void {
  const prev = state.selectedSessionId
  if (id === null) {
    // Clear the target FIRST so any synchronous re-render shows the fallback,
    // THEN collapse the mobile spoke so the back stack matches the screen. This
    // is the out-of-band clear path (e.g. an agent exit) — see
    // `unwindMobileSpoke`. Desktop stays on "home", so the unwind no-ops there.
    setState({
      selectedTarget: null,
      selectedSessionId: null,
      changes: emptyChanges(),
    })
    // Drop the previous session's changed-files subscription; there is no global
    // watch to clear, so the cross-client clobber is gone by construction.
    switchChangesSubscription(prev, null)
    writeSelectionHash()
    unwindMobileSpoke()
    return
  }
  setState({
    selectedTarget: { kind: "agent", sessionId: id },
    selectedSessionId: id,
    // Re-selecting the same session keeps its loaded data; a real switch enters
    // the loading window so the pane shows a spinner, not the previous session's
    // files.
    changes: prev === id ? state.changes : loadingChanges(id),
  })
  // Move the per-session changed-files subscription, THEN fetch — subscribing
  // before the GET means an invalidation that races the fetch is never missed.
  switchChangesSubscription(prev, id)
  writeSelectionHash()
  if (prev !== id) loadChanges(id)
}

// Select one of a session's companion terminals as the streamed target. The
// owning session id is retained so session-scoped UI keeps resolving.
export function selectTerminal(terminalId: string, sessionId: string): void {
  const prev = state.selectedSessionId
  setState({
    selectedTarget: { kind: "terminal", terminalId, sessionId },
    selectedSessionId: sessionId,
    // Switching from the agent to one of its own terminals keeps the same
    // session's loaded changes; only a different session enters loading.
    changes: prev === sessionId ? state.changes : loadingChanges(sessionId),
  })
  // The changed files belong to the SESSION, so subscribe/fetch the parent
  // session even when a companion terminal is the streamed target.
  switchChangesSubscription(prev, sessionId)
  writeSelectionHash()
  if (prev !== sessionId) loadChanges(sessionId)
}

// Spawn a new companion terminal for a session via REST (Phase 5). The 201 reply
// carries the new terminal id, so we focus it immediately — opening its PTY
// socket (`TerminalPane`) — rather than waiting for a `terminal_created` frame.
// The terminal also lands in the spine via the `sessions.changed` refetch, which
// fills in its label/status; focusing first is safe because the PTY socket only
// needs the ids the create returned. A failure surfaces as a toast.
export function createTerminal(sessionId: string): void {
  terminalsApi
    .create(sessionId)
    .then((created) => selectTerminal(created.terminal_id, sessionId))
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not create the terminal.",
      ),
    )
}

// Open the close-terminal confirmation dialog for a companion terminal. The TUI
// always confirms before killing a terminal's running process, so the web does
// too (the ✕ no longer deletes on a single click).
export function openDeleteTerminal(terminalId: string): void {
  setState({ deleteTerminalTarget: terminalId })
}

export function closeDeleteTerminal(): void {
  setState({ deleteTerminalTarget: null })
}

// Close (delete) a companion terminal via REST (Phase 5). The endpoint is nested
// under the owning session, so resolve the parent session id from the spine; a
// terminal that already vanished (no owner) is a no-op. The terminal is removed
// from the workspace spine, and if it was the focused target the selection clears
// via the spine prune in `applySpine` (driven by the `sessions.changed` refetch).
// A failure surfaces as a toast.
export function deleteTerminal(terminalId: string): void {
  const sessionId = state.spine?.sessions.find((s) =>
    s.terminals.some((t) => t.id === terminalId),
  )?.id
  if (sessionId === undefined) return
  terminalsApi
    .remove(sessionId, terminalId)
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not close the terminal.",
      ),
    )
}

// Open the discard-confirmation dialog for an unstaged file. The TUI confirms
// every discard because it's destructive — an untracked file is deleted, a
// tracked one loses its working-tree changes. The web mirrors that.
export function openDiscard(target: DiscardTarget): void {
  setState({ discardTarget: target })
}

export function closeDiscard(): void {
  setState({ discardTarget: null })
}

// Ask the server to discard a file's working-tree changes. The server re-derives
// the tracked/untracked distinction from live git status and rejects the command
// if the file is staged, so this never trusts the client about the destructive
// outcome.
export function discardFile(sessionId: string, path: string): void {
  git
    .discard(sessionId, path)
    .catch((e) => toast.error(e instanceof Error ? e.message : "discard failed"))
}

export function openCommit(sessionId: string): void {
  setState({ commitTarget: sessionId, commitDraft: "" })
}

export function closeCommit(): void {
  setState({ commitTarget: null, commitDraft: "" })
}

export function setCommitDraft(text: string): void {
  setState({ commitDraft: text })
}

// Open the code-editor overlay for a session. Selecting the session first points
// the engine's changed-files watch at its worktree so the editor's file list
// populates from the same broadcast the changes pane uses. `initialPath` (from a
// per-file affordance) auto-loads that file; `mode` chooses the opening view —
// "diff" when a changed file is clicked (show its diff first), "file" otherwise.
export function openEditor(
  sessionId: string,
  initialPath: string | null = null,
  mode: EditorViewMode = "file",
): void {
  if (state.selectedSessionId !== sessionId) selectSession(sessionId)
  setState({ editorTarget: { sessionId, initialPath, initialMode: mode } })
}

export function closeEditor(): void {
  setState({ editorTarget: null })
}

export function openDelete(sessionId: string): void {
  setState({ deleteTarget: sessionId })
}

export function closeDelete(): void {
  setState({ deleteTarget: null })
}

// Ask the server to delete an agent session. `deleteWorktree` opts into the
// destructive removal of the git worktree on disk (default off in the UI).
export function deleteSession(sessionId: string, deleteWorktree: boolean): void {
  sessionsApi
    .remove(sessionId, deleteWorktree)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not delete the session."),
    )
}

// Open the rename dialog for a session, pre-filling the current custom title
// (empty when none, so the placeholder shows the branch name).
export function openRename(sessionId: string): void {
  const session = state.spine?.sessions.find((s) => s.id === sessionId)
  setState({ renameTarget: sessionId, renameDraft: session?.title ?? "" })
}

export function closeRename(): void {
  setState({ renameTarget: null, renameDraft: "" })
}

export function setRenameDraft(raw: string): void {
  // Sanitize like the new-agent input: a custom title is validated as an agent
  // name server-side, so keep the dialog from accepting characters the server
  // would reject. Empty stays empty (clears the title back to the branch name).
  setState({ renameDraft: sanitizeAgentName(raw) })
}

// Ask the server to set a session's display title. An empty title clears it
// back to the branch name; a non-empty title is validated server-side. Resolves
// `true` on success, `false` (after toasting) on failure, so the rename dialog can
// stay open and preserve the user's input when the PATCH is rejected.
export async function renameSession(
  sessionId: string,
  title: string,
): Promise<boolean> {
  try {
    await sessionsApi.patch(sessionId, { title })
    return true
  } catch (e) {
    toast.error(e instanceof Error ? e.message : "Could not rename the session.")
    return false
  }
}

// Submit the rename dialog, closing it only once the PATCH succeeds. On failure
// the dialog stays open (the error is toasted) so the user does not lose the name
// they typed and can retry or cancel.
export async function submitRename(): Promise<void> {
  const id = state.renameTarget
  if (!id) return
  if (await renameSession(id, state.renameDraft.trim())) closeRename()
}

// Open the change-provider dialog for a session. The dialog pre-selects the
// session's current provider from the ViewModel.
export function openChangeProvider(sessionId: string): void {
  setState({ changeProviderTarget: sessionId })
}

export function closeChangeProvider(): void {
  setState({ changeProviderTarget: null })
}

// Whether `provider` is in the bootstrap document's configured provider list. The
// server re-validates authoritatively, but checking here first avoids firing a
// PATCH that the server will reject — important for the multi-field project PATCH,
// where a bad provider rejected mid-sequence would leave earlier fields (rename,
// auto-reopen) already committed (the PATCH is not atomic across independent
// fields). Empty list (pre-bootstrap) treats every provider as unconfigured.
function providerIsConfigured(provider: string): boolean {
  return (state.bootstrap?.available_providers ?? []).includes(provider)
}

// Ask the server to swap which provider a session uses. The provider is validated
// against the configured list up front (the server re-validates), persisted for
// the next launch, with the outcome (swapped / already-uses-it / still-running)
// reported on the status stream. Resolves `true` on success, `false` (after
// toasting) on a rejected/invalid provider so the dialog can stay open.
export async function changeAgentProvider(
  sessionId: string,
  provider: string,
): Promise<boolean> {
  if (!providerIsConfigured(provider)) {
    toast.error(`Provider "${provider}" is not configured.`)
    return false
  }
  try {
    await sessionsApi.patch(sessionId, { provider })
    return true
  } catch (e) {
    toast.error(e instanceof Error ? e.message : "Could not change the provider.")
    return false
  }
}

// Toggle a session's auto-reopen preference (PATCH `auto_reopen`). Shared by the
// desktop sidebar and the mobile session menu so the two surfaces never drift.
export function toggleSessionAutoReopen(
  sessionId: string,
  enabled: boolean,
): void {
  sessionsApi
    .patch(sessionId, { auto_reopen: enabled })
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not update auto-reopen.",
      ),
    )
}

// Ask the server to reconnect (relaunch) an agent. `force` starts a fresh
// session with no resume args (the TUI's force-reconnect); the default resumes
// the prior conversation when the provider supports it. Focus the session and
// bump `terminalEpoch` so the pane remounts and re-subscribes — the reconnect
// swaps in a new server-side provider, and the previously-attached forwarder is
// dead, so even an already-focused pane must re-issue `subscribe`. The server
// defers that subscribe until the freshly launched provider comes up.
export function reconnectSession(sessionId: string, force: boolean): void {
  sessionsApi
    .reconnect(sessionId, force)
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not reconnect the session.",
      ),
    )
  setState({
    selectedTarget: { kind: "agent", sessionId },
    selectedSessionId: sessionId,
    terminalEpoch: state.terminalEpoch + 1,
  })
}

export function openGlobalEnv(): void {
  setState({ globalEnvOpen: true })
}

export function closeGlobalEnv(): void {
  setState({ globalEnvOpen: false })
}

export function saveGlobalEnv(env: Record<string, string>): void {
  configApi
    .persistGlobalEnv(env)
    .catch((e) =>
      toast.error(
        e instanceof Error ? e.message : "Could not save the global environment.",
      ),
    )
}

export function openProjectSettings(projectId: string): void {
  setState({ projectSettingsTarget: projectId })
}

export function closeProjectSettings(): void {
  setState({ projectSettingsTarget: null })
}

// Open the agent-scoped startup-command editor. The target is the SESSION id; the
// dialog resolves and edits that agent's PROJECT startup command (startup command
// is project-scoped — there is no per-agent startup command).
export function openAgentStartupCommand(sessionId: string): void {
  setState({ agentStartupCommandTarget: sessionId })
}

export function closeAgentStartupCommand(): void {
  setState({ agentStartupCommandTarget: null })
}

// Open the agent-scoped environment editor. The target is the SESSION id; the
// dialog resolves and edits that agent's PROJECT env (env is project-scoped — it
// applies to every agent and terminal in the project).
export function openAgentEnv(sessionId: string): void {
  setState({ agentEnvTarget: sessionId })
}

export function closeAgentEnv(): void {
  setState({ agentEnvTarget: null })
}

// Open the startup-command log viewer for an agent and fetch its log files (with
// the newest file's contents pre-loaded). A reply is ignored once the viewer has
// closed or retargeted, so a late frame can't repopulate a stale viewer (the
// browse/attach-worktree precedent).
export function openStartupLogs(sessionId: string): void {
  setState({
    startupLogsTarget: sessionId,
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsError: null,
    startupLogsLoading: true,
  })
  sessionsApi
    .startupLogs(sessionId)
    .then((res) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({
        startupLogsEntries: res.entries,
        startupLogsSelected: res.selected,
        startupLogsError: null,
        startupLogsLoading: false,
      })
    })
    .catch((e) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({
        startupLogsLoading: false,
        startupLogsError:
          e instanceof Error
            ? e.message
            : "Could not load the startup command logs.",
      })
    })
}

// Switch the viewer to a different log file (fetches that file's contents).
export function selectStartupLog(name: string): void {
  const sessionId = state.startupLogsTarget
  if (!sessionId) return
  setState({ startupLogsLoading: true, startupLogsError: null })
  sessionsApi
    .startupLogContent(sessionId, name)
    .then((res) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({ startupLogsSelected: res, startupLogsLoading: false })
    })
    .catch((e) => {
      if (state.startupLogsTarget !== sessionId) return
      setState({
        startupLogsLoading: false,
        startupLogsError:
          e instanceof Error
            ? e.message
            : "Could not read the startup command log.",
      })
    })
}

export function closeStartupLogs(): void {
  setState({
    startupLogsTarget: null,
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsLoading: false,
    startupLogsError: null,
  })
}

// Re-run the agent's project startup command in its worktree (the TUI's
// `rerun-startup-command-on-agent`). The server runs it off-thread and reports
// busy/success/failure on the status stream — nothing to do here but fire the
// command and surface a transport/validation error if the request is rejected.
export function rerunStartupCommand(sessionId: string): void {
  sessionsApi
    .rerunStartupCommand(sessionId)
    .catch((e) =>
      toast.error(
        e instanceof Error
          ? e.message
          : "Could not rerun the startup command.",
      ),
    )
}

export function openProjectInfo(projectId: string): void {
  setState({ projectInfoTarget: projectId })
}

export function closeProjectInfo(): void {
  setState({ projectInfoTarget: null })
}

// Browse a directory for the add-project picker over REST (replaces the retired
// `/ws` `browse_dir` → `dir_entries` round-trip). A null path starts at $HOME.
// The reply is ignored once the dialog has closed so a late response can't
// repopulate a closed picker.
function runBrowse(path: string | null): void {
  browseApi
    .browse(path)
    .then((res) => {
      if (!state.addProjectOpen) return
      setState({
        browsePath: res.path,
        browseEntries: res.entries,
        browseLoading: false,
      })
    })
    .catch((e) => {
      if (!state.addProjectOpen) return
      setState({ browseEntries: [], browseLoading: false })
      toast.error(
        e instanceof Error ? e.message : "Could not browse the directory.",
      )
    })
}

export function openAddProject(): void {
  setState({ addProjectOpen: true, browseLoading: true, browseEntries: [] })
  runBrowse(null) // start at $HOME
}

export function closeAddProject(): void {
  setState({ addProjectOpen: false, projectPathInspection: null })
}

export function browseDir(path: string | null): void {
  // Navigating away abandons any pending/resolved branch inspection so a late
  // reply for the old selection can't resurface in the new directory.
  setState({ browseLoading: true, projectPathInspection: null })
  runBrowse(path)
}

// Fire the branch pre-flight for a selected git repo, mirroring the TUI's
// `add_project`, which inspects the current branch before adding. The reply
// fills `projectPathInspection` via `onProjectPathInspection`; the dialog shows
// a warning step when it carries one. Runs in the click handler that selects the
// repo — never an effect — like `openAttachWorktree` kicks off its listing.
export function inspectProjectPath(path: string): void {
  setState({
    projectPathInspection: {
      path,
      currentBranch: null,
      warning: null,
      error: null,
      loading: true,
    },
  })
  // Resolve over REST (replaces the retired `/ws` `inspect_project_path` reply).
  // Ignore a stale reply whose path no longer matches the pending inspection (the
  // user picked a different repo, or the dialog closed) so a late frame can never
  // repopulate a closed/changed selection.
  projectsApi
    .inspectPath(path)
    .then((res) => {
      if (state.projectPathInspection?.path !== path) return
      setState({
        projectPathInspection: {
          path,
          currentBranch: res.current_branch,
          warning: res.warning,
          error: null,
          loading: false,
        },
      })
    })
    .catch((e) => {
      if (state.projectPathInspection?.path !== path) return
      setState({
        projectPathInspection: {
          path,
          currentBranch: null,
          warning: null,
          error: e instanceof Error ? e.message : "Could not inspect the path.",
          loading: false,
        },
      })
    })
}

// Drop any pending/resolved inspection (e.g. the user deselected the repo).
export function clearProjectInspection(): void {
  setState({ projectPathInspection: null })
}

export function addProject(path: string, name: string): void {
  projectsApi
    .create({ path, name })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not add the project."),
    )
}

// Check out the repo's default branch first, then add it — the TUI's
// "Check Out & Add" path. Only offered for the Known warning (the server
// re-validates and rejects otherwise). The switch + add run server-side through
// the worker chain; the status stream reports the outcome.
export function addProjectCheckoutDefault(path: string, name: string): void {
  projectsApi
    .create({ path, name, checkout_default: true })
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not add the project."),
    )
}

export function openRemoveProject(projectId: string): void {
  setState({ removeProjectTarget: projectId })
}

export function closeRemoveProject(): void {
  setState({ removeProjectTarget: null })
}

export function removeProject(projectId: string): void {
  projectsApi
    .remove(projectId)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not remove the project."),
    )
}

// Update a project's settings (provider / auto-reopen / startup-command / env)
// in one tri-state PATCH. The caller (ProjectSettingsDialog) includes only the
// fields that changed; an omitted field is left untouched, `null` clears it.
export async function updateProjectSettings(
  projectId: string,
  patch: PatchProjectBody,
): Promise<boolean> {
  // Empty patch (nothing changed) is a successful no-op — let the dialog close.
  if (Object.keys(patch).length === 0) return true
  // Validate a provider SET (a non-null provider) up front: the PATCH dispatches
  // its fields as independent wire sub-commands with no rollback, so a provider the
  // server rejects mid-sequence would leave the earlier fields already committed.
  // Catching it here (and the backend's matching up-front check) keeps a bad
  // provider from partially applying. `null` clears the provider and needs no check.
  if (
    patch.provider != null &&
    !providerIsConfigured(patch.provider)
  ) {
    toast.error(`Provider "${patch.provider}" is not configured.`)
    return false
  }
  try {
    await projectsApi.patch(projectId, patch)
    return true
  } catch (e) {
    toast.error(
      e instanceof Error ? e.message : "Could not update project settings.",
    )
    return false
  }
}

// Refresh a project's source checkout from remote (the TUI's
// `refresh_selected_project`). The server resolves the project, runs the pull
// against its source checkout, and reports busy/success/failure on the status
// stream — nothing to do here but fire the command.
export function pullProject(projectId: string): void {
  projectsApi
    .pull(projectId)
    .catch((e) => toast.error(e instanceof Error ? e.message : "pull failed"))
}

// Open the confirm dialog for switching a project's source checkout back to its
// default branch. The actual git work happens server-side after the user
// confirms (the checkout moves HEAD, so it is gated behind a confirmation the
// TUI's deliberate palette action does not need).
export function openCheckoutDefaultBranch(projectId: string): void {
  setState({ checkoutDefaultBranchTarget: projectId })
}

export function closeCheckoutDefaultBranch(): void {
  setState({ checkoutDefaultBranchTarget: null })
}

// Tell the server to inspect and check out the project's default branch. The
// server reports the outcome (switched / already on it / can't determine) on
// the command result, so there is nothing to do here but fire the command.
export function checkoutDefaultBranch(projectId: string): void {
  projectsApi
    .checkoutDefault(projectId)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "checkout failed")
    )
}

// Open the attach-worktree dialog for a project and immediately request its
// managed-worktree listing (the server classifies in spawn_blocking). The
// listing reply fills `attachWorktreeEntries` via `onProjectWorktrees`. Runs in
// the click handler that opens the dialog — never an effect — mirroring how
// `openAddProject` kicks off its browse.
export function openAttachWorktree(projectId: string): void {
  setState({
    attachWorktreeTarget: projectId,
    attachWorktreeEntries: [],
    attachWorktreeLoading: true,
  })
  // Fetch the managed-worktree listing over REST (replaces the retired `/ws`
  // `list_project_worktrees` → `project_worktrees` reply). Ignore a stale reply
  // if the dialog closed (or switched projects) before it arrived.
  projectsApi
    .worktrees(projectId)
    .then((res) => {
      if (state.attachWorktreeTarget !== projectId) return
      setState({
        attachWorktreeEntries: res.entries,
        attachWorktreeLoading: false,
      })
    })
    .catch((e) => {
      if (state.attachWorktreeTarget !== projectId) return
      setState({ attachWorktreeEntries: [], attachWorktreeLoading: false })
      toast.error(
        e instanceof Error ? e.message : "Could not list the worktrees.",
      )
    })
}

export function closeAttachWorktree(): void {
  setState({
    attachWorktreeTarget: null,
    attachWorktreeEntries: [],
    attachWorktreeLoading: false,
  })
}

// Ask the server to adopt a managed worktree as a new agent. The server
// re-validates the path against a fresh classification (never trusting this
// list) and validates `name` as a display name, then dispatches the create
// worker — the outcome (busy/success/failure) arrives on the status stream.
export function attachWorktree(
  projectId: string,
  worktreePath: string,
  name: string,
): void {
  armCreateFocus(projectId)
  sessionsApi
    .create({ kind: "from_worktree", project_id: projectId, worktree_path: worktreePath, name })
    .catch((e) => toastCreateError(e, "Could not attach the worktree."))
}

// Open the new-agent dialog. The checkbox starts checked when
// `randomize_agent_names_by_default` is set (mirroring the TUI prompt, which
// pre-checks when opened with no initial name); in that case we request a name
// right away so the input previews it. This runs in the click handler that opens
// the dialog — never an effect — so there is no set-state-in-effect.
export function openCreateAgent(projectId: string): void {
  openNameDialog({ kind: "new", projectId })
}

// Open the name dialog in fork mode for an existing session. Reuses the exact
// new-agent UX (sanitized input, pet-name checkbox, generated-name plumbing);
// only the dispatch target differs. Unlike create, a fork REQUIRES a name (the
// server rejects an empty fork), so the dialog's Fork button is disabled while
// the input is empty.
export function openForkAgent(sessionId: string): void {
  openNameDialog({ kind: "fork", sessionId })
}

// Open the name dialog in "from PR" mode for a project. Reuses the new-agent
// name UX (sanitized input, pet-name checkbox, generated-name plumbing) and adds
// a PR-reference field; on submit it dispatches `create_agent_from_pr`. Mirrors
// the TUI's `new-agent-from-pr` flow, which resolves the PR then names the agent.
export function openCreateAgentFromPr(projectId: string): void {
  openNameDialog({ kind: "pr", projectId })
}

// Shared opener for all modes of the name dialog. Pre-checks the randomize
// default and requests a name right away so the input previews it, exactly like
// the TUI prompt — EXCEPT in PR mode: the TUI seeds the PR's head branch as the
// name and never randomizes there (a pet name would become the branch the PR
// head is fetched into). The web doesn't know the head branch until after the
// lookup, so PR mode opens blank and the server's head-branch fallback applies.
// Runs in the click handler that opens the dialog — never an effect — so there
// is no set-state-in-effect.
// Request a fresh pet name for the new-agent dialog over REST (replaces the
// retired `/ws` `generate_agent_name` → `agent_name` reply). The TUI fills the
// input with the generated name (that fill IS the preview) and remembers it so a
// later uncheck can tell "still the generated name" from "user-edited". We mirror
// that: fill the draft and stash the name. Ignored if the dialog closed or the
// user unchecked the box before the reply landed (a stale reply must not refill).
// A failure stops the spinner so the user can type a name by hand.
function requestAgentName(): void {
  browseApi
    .agentName()
    .then((res) => {
      if (state.createAgentTarget !== null && state.createAgentRandomize) {
        setState({
          createAgentDraft: res.name,
          createAgentGeneratedName: res.name,
          createAgentNamePending: false,
        })
      }
    })
    .catch(() => {
      if (state.createAgentTarget !== null) {
        setState({ createAgentNamePending: false })
      }
    })
}

function openNameDialog(target: CreateAgentTarget): void {
  const randomize =
    target.kind !== "pr" &&
    (state.bootstrap?.randomize_agent_names_by_default ?? false)
  setState({
    createAgentTarget: target,
    createAgentDraft: "",
    createAgentRandomize: randomize,
    createAgentGeneratedName: null,
    createAgentNamePending: randomize,
    createAgentPrInput: "",
  })
  if (randomize) requestAgentName()
}

export function closeCreateAgent(): void {
  setState({
    createAgentTarget: null,
    createAgentDraft: "",
    createAgentRandomize: false,
    createAgentGeneratedName: null,
    createAgentNamePending: false,
    createAgentPrInput: "",
  })
}

// Update the PR-reference field. Free text — unlike the agent name, this is NOT
// sanitized (a PR URL contains slashes, colons, etc.); the server parses it.
export function setCreateAgentPrInput(raw: string): void {
  setState({ createAgentPrInput: raw })
}

// Update the input as the user types, sanitizing live (space -> dash, drop
// disallowed chars, etc.) exactly like the TUI char map. Editing away from the
// generated name clears the remembered name so a later uncheck keeps the edits.
export function setCreateAgentDraft(raw: string): void {
  const draft = sanitizeAgentName(raw)
  const generated =
    draft === state.createAgentGeneratedName ? state.createAgentGeneratedName : null
  setState({ createAgentDraft: draft, createAgentGeneratedName: generated })
}

// Toggle the "Use randomized pet name" checkbox with the TUI's exact semantics:
//   ON  -> request a fresh name (the reply fills the input via `onAgentName`).
//   OFF -> clear the input ONLY if it still equals the generated name; otherwise
//          keep the user's edits. Either way, forget the generated name.
export function toggleCreateAgentRandomize(): void {
  if (!state.createAgentRandomize) {
    setState({ createAgentRandomize: true, createAgentNamePending: true })
    requestAgentName()
  } else {
    const keepText = state.createAgentDraft !== state.createAgentGeneratedName
    setState({
      createAgentRandomize: false,
      createAgentDraft: keepText ? state.createAgentDraft : "",
      createAgentGeneratedName: null,
      // Unchecking abandons any in-flight request; its reply is ignored by
      // `onAgentName` (randomize is false by then), so stop the spinner now.
      createAgentNamePending: false,
    })
  }
}

// Surface a create-action REST error as a toast, EXCEPT a 409 Conflict. A 409
// means the engine's in-flight create guard refused: it returns an `Ok`
// error-toned status that the engine ALSO broadcasts over `/ws` (scoped to this
// connection) and that the REST handler maps to 409. Toasting the 409 here would
// double up — the user would see two identical toasts for one refusal. The `/ws`
// status stream is the single surface for that case; every other status still
// toasts. Network failures (`status === 0`) and all other codes are surfaced.
function toastCreateError(e: unknown, fallback: string): void {
  if (e instanceof SessionsApiError && e.status === 409) return
  toast.error(e instanceof Error ? e.message : fallback)
}

// Ask the server to create a new agent in a project. An empty name lets the
// server auto-generate a branch name (the equivalent outcome to the TUI's
// generate-a-pet-name path). With the checkbox checked the input is effectively
// never empty, so the empty path is the unchecked-and-blank case.
export function createAgent(projectId: string, name: string): void {
  sessionsApi
    .create({ kind: "new", project_id: projectId, name })
    .catch((e) => toastCreateError(e, "Could not create the agent."))
}

// Ask the server to fork an existing session into a fresh branched worktree.
// Unlike create, a fork requires a non-empty name (the server rejects empty).
export function forkAgent(sessionId: string, name: string): void {
  sessionsApi
    .create({ kind: "fork", session_id: sessionId, name })
    .catch((e) => toastCreateError(e, "Could not fork the session."))
}

// Ask the server to create an agent checked out on a GitHub PR's head branch.
// `pr` is the raw reference (URL, `#123`, or `123`); the server resolves it via
// `gh pr view`. An empty `name` falls back to the PR head branch, matching the
// TUI prompt default. The lookup+create runs asynchronously: the command returns
// a busy status synchronously and the outcome arrives on the status stream.
export function createAgentFromPr(projectId: string, pr: string, name: string): void {
  sessionsApi
    .create({ kind: "from_pr", project_id: projectId, pr, name })
    .catch((e) => toastCreateError(e, "Could not create the agent from the PR."))
}

// Submit the name dialog: dispatch create, fork, or create-from-PR based on the
// current target, then close. Mirrors the TUI, where the same name prompt drives
// these flows.
export function submitNameDialog(name: string): void {
  const target = state.createAgentTarget
  if (!target) return
  if (target.kind === "new") {
    armCreateFocus(target.projectId)
    createAgent(target.projectId, name)
  } else if (target.kind === "fork") {
    // A fork lands in the same project as its source session; resolve it so the
    // focus diff is scoped to that project. If the source vanished from the
    // ViewModel, skip auto-focus rather than arming an unscoped token that could
    // grab any project's next new session.
    const projectId = state.spine?.sessions.find(
      (s) => s.id === target.sessionId,
    )?.project_id
    if (projectId) armCreateFocus(projectId)
    forkAgent(target.sessionId, name)
  } else {
    armCreateFocus(target.projectId)
    createAgentFromPr(target.projectId, state.createAgentPrInput.trim(), name)
  }
  closeCreateAgent()
}

// Optimistically reorder a project's sessions, then tell the server. `orderedIds`
// MUST be the complete ordered set of that project's session ids — the server
// validates it as a strict permutation and rejects partial/stale sets. The
// overlay clears when the next spine confirms the order (or on error).
export function reorderSessions(projectId: string, orderedIds: string[]): void {
  setState({ pendingSessionOrder: { projectId, ids: orderedIds } })
  sessionsApi
    .reorder(projectId, orderedIds)
    .catch((e) => {
      // A rejected reorder will never be reconciled by a spine (the server never
      // persisted this order), so the optimistic overlay would otherwise linger
      // forever — leaving the sidebar showing an order the server doesn't have and
      // compounding on the next drag. Clear the order overlays so the UI snaps back
      // to the authoritative spine order, then surface the failure.
      setState(clearPendingOrders())
      toast.error(
        e instanceof Error ? e.message : "Could not reorder the sessions.",
      )
    })
}

// Sort every project's sessions by the chosen key, mirroring the TUI palette
// commands sort-agents-by-{updated,created,name}. There's no dedicated web sort
// state: for each project we compute the sorted id order (sortedSessionIds, which
// mirrors the TUI comparators exactly) and send the EXISTING `reorder_sessions`
// command, which the server persists into the same shared order the TUI uses —
// so the two surfaces stay in sync by construction.
//
// We deliberately DON'T set the optimistic `pendingSessionOrder` overlay here.
// That overlay holds a single project; a sort touches N projects, so an overlay
// could only cover one of them and would leave the rest snapping anyway. The
// spine echo arrives within tens of milliseconds, so the brief reflow is
// acceptable and keeps the single-project drag overlay invariant untouched.
// Projects with fewer than two sessions are skipped — sorting them is a no-op
// that would only churn the wire.
export function sortAgents(by: SortKey): void {
  const sessions = state.spine?.sessions ?? []
  const projects = state.spine?.projects ?? []
  // A sort supersedes any in-flight drag: drop its overlay up front, or a
  // superseded drag order would linger on screen until something else clears
  // it (the overlay only retires on match/error/disconnect).
  setState(clearPendingOrders())
  for (const project of projects) {
    const projectSessions = sessions.filter((s) => s.project_id === project.id)
    if (projectSessions.length < 2) continue
    const orderedIds = sortedSessionIds(projectSessions, by)
    sessionsApi
      .reorder(project.id, orderedIds)
      .catch((e) =>
        toast.error(
          e instanceof Error ? e.message : "Could not reorder the sessions.",
        ),
      )
  }
}

// Optimistically reorder the projects, then tell the server. `orderedIds` MUST
// be the complete ordered set of ALL project ids (both with and without agents);
// the server validates it as a strict permutation. The overlay clears when the
// next spine confirms the order (or on error).
export function reorderProjects(orderedIds: string[]): void {
  setState({ pendingProjectOrder: orderedIds })
  projectsApi
    .reorder(orderedIds)
    .catch((e) => {
      // As with sessions: a rejected reorder is never reconciled by a spine, so
      // the optimistic overlay would persist indefinitely. Clear it back to the
      // authoritative order before surfacing the error.
      setState(clearPendingOrders())
      toast.error(
        e instanceof Error ? e.message : "Could not reorder the projects.",
      )
    })
}

export function setPaletteOpen(open: boolean): void {
  setState({ paletteOpen: open })
}

// Run a macro by name on the focused PTY. Since Phase 5 the web no longer sends a
// server-side `run_macro` command: it resolves the macro's text from the bootstrap
// document, applies the newline→Alt+Enter transform (`macroPayloadBytes`, an exact
// port of the engine's), and writes the payload straight to the active PTY socket
// as stdin — the same socket the focused terminal pane drives. The macro picker is
// already filtered to the focused surface, so the active socket IS the macro's
// target. No-op if the macro is unknown or no terminal is focused (no active
// socket). The text is pasted WITHOUT a trailing submit, mirroring the TUI: the
// user reviews it in the prompt and presses Enter to send.
export function runMacro(name: string): void {
  const macro = (state.bootstrap?.macros ?? []).find((m) => m.name === name)
  if (!macro) return
  // Defensive: only inject when a terminal is actually focused. During a focus
  // switch the outgoing pane may not have cleared its registration yet; without
  // a selected target the active socket is stale, and writing to it would paste
  // the macro into the wrong (just-detached) PTY.
  if (state.selectedTarget === null) return
  const pty = getActivePtySocket()
  if (pty === null) return
  pty.sendInput(macroPayloadBytes(macro.text))
}

// Open the macro-editor dialog, seeding the draft from the current bootstrap
// macros (a fresh copy so edits don't mutate the shared model). Runs in the
// click/palette handler that opens the dialog — never an effect.
export function openMacrosDialog(): void {
  const macros = state.bootstrap?.macros ?? []
  setState({
    macrosDialogOpen: true,
    macrosDraft: macros.map((m) => ({ ...m })),
  })
}

export function closeMacrosDialog(): void {
  setState({ macrosDialogOpen: false, macrosDraft: [] })
}

// Persist the draft wholesale via `update_macros`. The server validates
// (empty/duplicate names, empty text, unknown surface) and reports the outcome
// on the status lane; a config reload emits `config.changed`, refetching
// `bootstrap.macros`. The dialog closes optimistically — a rejection surfaces as
// an error toast, and reopening re-seeds from the (unchanged) bootstrap.
export function saveMacros(macros: MacroView[]): void {
  // `update_macros` is a WHOLESALE replace of the entire `[macros]` map. Before
  // the bootstrap document has loaded, `openMacrosDialog` seeded an EMPTY draft,
  // so saving would wipe the server's macros. Refuse until we hold the
  // authoritative list (the Save button is also disabled in this window).
  if (state.bootstrap === null) {
    toast.error("Macros aren't loaded yet. Try again in a moment.")
    return
  }
  configApi
    .updateMacros(macros)
    .catch((e) =>
      toast.error(e instanceof Error ? e.message : "Could not save the macros."),
    )
  closeMacrosDialog()
}

// Mobile hub-&-spoke navigation. Moving INTO a spoke ("terminal" or "changes")
// pushes a history entry so the hardware/browser Back button unwinds the stack
// one screen at a time (see the popstate listener above). Navigating to "home"
// is a programmatic return: rather than just flipping state (which would leave
// the pushed spoke entries dangling), it routes through `unwindMobileSpoke` so
// the history depth collapses to match — keeping the back stack honest for any
// future caller. Re-navigating to the screen we're already on is a no-op so we
// never stack duplicate history entries (e.g. switching sessions while already
// on the terminal screen must not deepen the back stack). The comparison reads
// the LATEST `state.mobileScreen`, so a tap that races a pending popstate still
// sees the up-to-date screen and won't double-push.
export function mobileNavigate(screen: MobileScreen): void {
  if (screen === state.mobileScreen) return
  if (screen === "home") {
    unwindMobileSpoke()
    return
  }
  setState({ mobileScreen: screen })
  history.pushState({ duxMobile: screen }, "")
}

export function reconnect(): void {
  // The events socket gives up after MAX_RECONNECT_ATTEMPTS and signals "failed";
  // a manual reconnect restarts it. connect() resets closedByUser/attempts/delay,
  // so it is safe to call on an exhausted socket.
  eventsSocket.connect()
}

// Update the expanded sidebar width during a drag. Pass `persist` on release to
// write the final value to localStorage.
export function setSidebarWidth(width: string, persist = false): void {
  setState({ sidebarWidth: width })
  if (persist) {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, width)
  }
}

// The Changes pane's effective visibility (desktop): the per-session override if
// set, else the config default from the bootstrap document, else visible (the
// pre-load window before the first bootstrap fetch lands).
export function changesPaneVisible(s: DuxState): boolean {
  return s.changesPaneOverride ?? s.bootstrap?.show_changes_pane ?? true
}

// Toggle the Changes pane (the Ctrl+K "toggle-remove-git-pane" command and the
// Changes actions menu) and persist the choice. The override is set
// optimistically for an instant response; the server writes
// config.ui.show_changes_pane and emits `config.changed`, the refetched bootstrap
// document carries the confirmed value, and `applyBootstrap` drops the override
// so config is the single source of truth across every connected client.
export function toggleChangesPane(): void {
  const next = !changesPaneVisible(state)
  setState({ changesPaneOverride: next })
  configApi
    .setChangesPaneVisible(next)
    .catch((e) => {
      // Roll the optimistic override back so the pane doesn't strand in the
      // toggled state when the persist fails.
      setState({ changesPaneOverride: null })
      toast.error(
        e instanceof Error ? e.message : "Could not toggle the Changes pane.",
      )
    })
}
