import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { sanitizeAgentName } from "./agentName"
import { git } from "./git"
import {
  type AuthState,
  type MeBody,
  loginErrorMessage,
  LOGIN_NETWORK_MESSAGE,
  parseRetryAfter,
  phaseFromMe,
  unreachableRetryDelay,
} from "./auth"
import { ordersMatch } from "./reorder"
import type { StatusLineState } from "./statusLine"
import { sortedSessionIds, type SortKey } from "./sortSessions"
import { DuxSocket } from "./ws"
import type {
  BranchWarningView,
  ConnState,
  DirEntryView,
  MacroView,
  ProjectWorktreeEntryView,
  ViewModel,
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

// A tiny external store backed by `useSyncExternalStore`. A single module-level
// `DuxSocket` instance feeds it: ViewModel updates, connection state, and
// command/error results (surfaced as `statusLine`, carrying the engine tone so
// the StatusBar renders 1:1 with the TUI's status line). The PTY byte stream is
// NOT kept in React state — the terminal attaches to `socket.onPtyBytes`
// directly.

export interface DuxState {
  viewModel: ViewModel | null
  conn: ConnState
  // The login/auth-state machine. Starts "checking" while the boot `/api/me`
  // round-trip is in flight; resolves to "disabled" (auth off — today's UX),
  // "authed" (a valid session), "anonymous" (auth on, no session → the login
  // screen), or "unreachable" (the boot probe network-failed → the retrying
  // reconnect screen; the store auto-retries). The app shell only renders once
  // the phase is "disabled" or "authed", so the WS connect (issued the moment we
  // learn auth is off or we're authed) always precedes the terminal's first
  // subscribe. See `bootAuth` below.
  auth: AuthState
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
  // The persistent statusline shown at the bottom of the shell, mirroring the
  // TUI 1:1. Both the synchronous command-result path and the async status
  // stream write here, keeping the engine's tone so the bar can color and
  // icon-tag the message (toasts are a separate, transient surface). An empty
  // message renders nothing.
  statusLine: StatusLineState
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
  // map, mirroring the TUI editor). Seeded from `viewModel.macros` on open so
  // there is no set-state-in-effect. Empty draft when closed.
  macrosDialogOpen: boolean
  macrosDraft: MacroView[]
  // Which screen the mobile shell is showing. Always "home" on desktop, which
  // ignores it. Only the mobile UI advances it past "home".
  mobileScreen: MobileScreen
  // Optimistic drag-and-drop ordering overlays (see `applyPendingOrders`). Each
  // is set the moment a drag ends and cleared once the server's next ViewModel
  // confirms the new order (or an error status arrives). Null when no reorder is
  // in flight, which is the overwhelmingly common case.
  pendingSessionOrder: PendingSessionOrder | null
  pendingProjectOrder: string[] | null
  // While an agent-create THIS client initiated is in flight, holds the session
  // ids that already existed when we submitted, plus the project the new agent
  // will land in. Agent creation is an async server job whose only completion
  // signal is a broadcast ViewModel (no per-client reply, no request/echo
  // correlation), so we recognize "our" new agent as the session id that appears
  // in `projectId` and wasn't in `knownIds`, then focus it — mirroring the TUI,
  // which jumps selection to a freshly created agent when its launch completes.
  // Only the client that armed this reacts, so other connected clients aren't
  // yanked off whatever they're viewing. Null when no create is awaiting focus.
  // See `armCreateFocus` and `focusNewlyCreatedSession`.
  pendingCreateFocus: { knownIds: string[]; projectId: string } | null
  sidebarWidth: string
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

let state: DuxState = {
  viewModel: null,
  conn: "connecting",
  auth: { phase: "checking", username: null, error: null, pending: false },
  selectedTarget: null,
  selectedSessionId: null,
  terminalEpoch: 0,
  statusLine: { tone: "info", message: "" },
  commitTarget: null,
  commitDraft: "",
  deleteTarget: null,
  deleteTerminalTarget: null,
  discardTarget: null,
  globalEnvOpen: false,
  projectSettingsTarget: null,
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
  editorTarget: null,
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

// The single socket instance for the whole app. Exported so components that
// talk to the PTY (terminal) or issue commands (palette) can use it directly.
export const socket = new DuxSocket(`ws://${location.host}/ws`)

socket.onViewModel = (vm) => {
  // Normalize at the boundary: `macros` is the newest ViewModel field, so an
  // older server snapshot that predates it arrives with the key absent. Default
  // it to `[]` here (the parse site) so the typed-required `viewModel.macros` is
  // a real array for every consumer — making the types.ts "defaults to []"
  // claim structurally true rather than relying on each read site to guard.
  const normalized: ViewModel = { ...vm, macros: vm.macros ?? [] }
  setState({
    viewModel: normalized,
    // Retire each optimistic order overlay once the server's order matches it;
    // until then keep showing the overlay so the row doesn't snap back during
    // the round-trip. A stale (non-matching) overlay is kept — a later ViewModel
    // confirming our reorder will clear it; an error status clears it outright.
    pendingSessionOrder: reconcilePendingSessionOrder(vm, state.pendingSessionOrder),
    pendingProjectOrder: reconcilePendingProjectOrder(vm, state.pendingProjectOrder),
  })
  // If an agent THIS client just created has now appeared, jump focus to it
  // (see `focusNewlyCreatedSession`). Run before the prune below: this only ever
  // selects a session present in `vm`, so the prune leaves it alone.
  focusNewlyCreatedSession(vm)
  // If the focused target vanished (an agent session was removed, or a
  // companion terminal exited and was pruned server-side), drop the selection
  // so the center pane shows the empty state instead of a dead terminal.
  pruneSelectionIfGone(vm)
}

// Drop the pending session-order overlay once the incoming ViewModel's session
// order for that project already equals the overlay; otherwise keep it.
function reconcilePendingSessionOrder(
  vm: ViewModel,
  pending: PendingSessionOrder | null,
): PendingSessionOrder | null {
  if (!pending) return null
  const serverIds = vm.sessions
    .filter((s) => s.project_id === pending.projectId)
    .map((s) => s.id)
  return ordersMatch(serverIds, pending.ids) ? null : pending
}

// Drop the pending project-order overlay once the incoming ViewModel's project
// order already equals the overlay; otherwise keep it.
function reconcilePendingProjectOrder(
  vm: ViewModel,
  pending: string[] | null,
): string[] | null {
  if (!pending) return null
  const serverIds = vm.projects.map((p) => p.id)
  return ordersMatch(serverIds, pending) ? null : pending
}

// Clear the selection when its target no longer exists in the latest ViewModel.
// Agents persist after exiting (their session stays, marked detached), so they
// only vanish on deletion; terminals are removed outright when their PTY exits.
function pruneSelectionIfGone(vm: ViewModel): void {
  const target = state.selectedTarget
  if (!target) return
  const stillExists =
    target.kind === "agent"
      ? vm.sessions.some((s) => s.id === target.sessionId)
      : vm.sessions.some((s) =>
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
// THIS client is creating, so the next ViewModel carrying a new id in `projectId`
// is recognized as our new agent and focused (see `focusNewlyCreatedSession`).
// Call this immediately before dispatching an agent-create command; it is wired
// into `submitNameDialog` (new/fork/from-PR) and `attachWorktree`. Re-arming
// overwrites any prior pending focus, so a fresh create supersedes an earlier
// one whose agent never arrived. Always pass the project the new agent will land
// in — the match is project-scoped, so a caller that cannot resolve the project
// must skip arming rather than pass a placeholder.
function armCreateFocus(projectId: string): void {
  const knownIds = (state.viewModel?.sessions ?? []).map((s) => s.id)
  setState({ pendingCreateFocus: { knownIds, projectId } })
}

// Focus the agent THIS client just created, the instant it shows up. With a
// pending-focus token armed (`armCreateFocus`), scan the incoming ViewModel for a
// session that wasn't known at submit time and lives in the expected project,
// select it (which points the changed-files watch at it; the focused TerminalPane
// subscribes its PTY on mount), and disarm. No-op — and cheap — when nothing is
// pending, the overwhelmingly common case. Other clients never armed a token, so
// they don't react: focus moves only on the client that initiated the create.
function focusNewlyCreatedSession(vm: ViewModel): void {
  const pending = state.pendingCreateFocus
  if (!pending) return
  const known = new Set(pending.knownIds)
  const created = vm.sessions.find(
    (s) => !known.has(s.id) && s.project_id === pending.projectId,
  )
  if (!created) return
  // Consume the token before selecting so a later ViewModel can't re-fire.
  setState({ pendingCreateFocus: null })
  selectSession(created.id)
}

socket.onConn = (conn) => {
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
  // Re-establish the server-side changed-files watch on every (re)connect. The
  // watch is server state that a dropped connection discards, so a reconnect
  // would otherwise leave the pane empty until the next manual selection. This
  // mirrors how the focused terminal re-attaches after a reconnect — it re-issues
  // its `subscribe` so the new server-side provider streams again.
  if (conn === "open" && state.selectedSessionId !== null) {
    socket.sendCommand("watch_changed_files", {
      session_id: state.selectedSessionId,
    })
  }
  // When the socket gives up (the reconnect loop exhausted its attempts) while
  // we believe we're authed, the cause may be a session that expired or was
  // revoked server-side: the gated `/ws` upgrade now 401s, which the browser
  // surfaces as an error+close, never an open — so the loop fails. Re-check
  // `/api/me`; a 401 means we really are logged out, so flip to the login screen
  // instead of showing the dead "Reconnect" UX. A still-authed `/api/me` means
  // it was a genuine network blip, so we leave "failed" in place (the user can
  // hit Reconnect). Event-driven: this fires only on the terminal "failed" edge.
  if (conn === "failed" && state.auth.phase === "authed") {
    void recheckAuthAfterFailure()
  }
}

// After the WS reconnect loop fails while authed, re-verify the session. A 401
// means the session is gone (expired/revoked): drop to the login screen. Any
// other outcome (still authed, or the probe itself failed) is treated as a
// transient network problem and left as "failed" so the Reconnect affordance
// stands. Never connects the socket here — that happens on a fresh login.
async function recheckAuthAfterFailure(): Promise<void> {
  try {
    const resp = await fetch("/api/me", { credentials: "same-origin" })
    if (resp.status === 401) {
      setState({
        auth: { phase: "anonymous", username: null, error: null, pending: false },
      })
    }
  } catch {
    // Probe failed too — keep "failed"; this was a network problem, not a logout.
  }
}

// Engine status/command events go to the status LINE only, 1:1 with the TUI —
// the TUI shows these in its status line and never as a separate transient
// notice. (Earlier these also fired a toast, so a single event surfaced twice;
// the status line is the single source of truth for engine-driven status.)
socket.onCommandResult = (status, error) => {
  if (error) {
    // A rejected reorder (stale/partial id set) comes back as an error here;
    // drop any optimistic overlay so the UI reverts to the server's order. The
    // error string IS the message and the tone is "error".
    setState({
      statusLine: { tone: "error", message: error },
      ...clearPendingClientIntent(),
    })
  } else if (status) {
    setState({ statusLine: { tone: status.tone, message: status.message } })
  }
}

socket.onError = (message) => {
  setState({
    statusLine: { tone: "error", message },
    ...clearPendingClientIntent(),
  })
}

// Reset both optimistic order overlays. Returned as a patch so callers can fold
// it into a single `setState`. Used on every error path so a rejected reorder
// snaps the UI back to the server's authoritative order.
function clearPendingOrders(): Partial<DuxState> {
  return { pendingSessionOrder: null, pendingProjectOrder: null }
}

// Clear every transient, optimistic client intent at once: the reorder overlays
// AND any pending create-focus. Used on the failure/teardown paths (command
// error, async error status, socket disconnect) where an in-flight create can no
// longer be trusted to resolve — a surviving `pendingCreateFocus` snapshot would
// otherwise mis-identify a later, unrelated session as the one we created. NOT
// folded into `clearPendingOrders` because user actions like sorting also clear
// the order overlays but must NOT cancel an in-flight create-focus.
function clearPendingClientIntent(): Partial<DuxState> {
  return { ...clearPendingOrders(), pendingCreateFocus: null }
}

// Asynchronous status/lifecycle events (background push/pull completing, an
// agent launch finishing or failing, a PTY exiting). These mirror the TUI's
// status line 1:1 — keep the latest in the status bar, no separate toast.
socket.onStatus = (tone, message) => {
  // An error-toned async status also voids any in-flight create-focus (the
  // create likely just failed) and unwinds any optimistic reorder overlay.
  const patch = tone === "error" ? clearPendingClientIntent() : {}
  setState({ statusLine: { tone, message }, ...patch })
}

// A freshly created terminal auto-focuses so the user lands on it immediately.
socket.onTerminalCreated = (sessionId, terminalId) => {
  selectTerminal(terminalId, sessionId)
}

socket.onCommitMessage = (sessionId, message) => {
  // The generated message is broadcast to every client and tagged with the
  // session it was generated for. Apply it ONLY when it matches the open
  // dialog's target; otherwise drop it (the dialog was closed or switched to a
  // different session) so one session's message never clobbers another's draft.
  if (state.commitTarget === sessionId) {
    setState({ commitDraft: message })
  }
}

socket.onCommitMessageSnapshot = (sessionId, message) => {
  // The connect snapshot re-delivers the LAST generated message so a reconnect
  // during a commit flow still fills the draft. Apply it conservatively: only
  // when this session's dialog is open AND the draft is still empty, so a stale
  // snapshot never clobbers an in-progress edit (or fills a dialog the user
  // never asked to generate into). The live `onCommitMessage` path handles the
  // normal case where the client stayed connected through generation.
  if (state.commitTarget === sessionId && state.commitDraft === "") {
    setState({ commitDraft: message })
  }
}

socket.onDirEntries = (path, entries, error) => {
  setState({ browsePath: path, browseEntries: error ? [] : entries, browseLoading: false })
  if (error) toast.error(error)
}

// The managed-worktree listing reply for the attach dialog. Ignore a stale
// reply if the dialog closed (or switched projects) before it arrived, so a
// late frame can never repopulate a closed dialog.
socket.onProjectWorktrees = (projectId, entries, error) => {
  if (state.attachWorktreeTarget !== projectId) return
  setState({ attachWorktreeEntries: error ? [] : entries, attachWorktreeLoading: false })
  if (error) toast.error(error)
}

// A freshly generated pet name for the new-agent dialog. The TUI fills the input
// with the generated name (that fill IS the preview) and remembers it so a later
// uncheck can tell "still the generated name" from "user-edited". We mirror that:
// fill the draft and stash the name. Ignored if the dialog closed or the user
// unchecked the box before the reply landed (a stale reply must not refill).
socket.onAgentName = (name) => {
  if (state.createAgentTarget !== null && state.createAgentRandomize) {
    setState({
      createAgentDraft: name,
      createAgentGeneratedName: name,
      createAgentNamePending: false,
    })
  }
}

// The add-project branch pre-flight reply. Ignore a stale reply whose path no
// longer matches the pending inspection (the user picked a different repo, or
// the dialog closed) so a late frame can never repopulate a closed/changed
// selection — same staleness guard as `onProjectWorktrees`.
socket.onProjectPathInspection = (path, currentBranch, warning, error) => {
  if (state.projectPathInspection?.path !== path) return
  setState({
    projectPathInspection: {
      path,
      currentBranch,
      warning,
      error,
      loading: false,
    },
  })
}

// Boot the auth-state machine, THEN connect the socket (the reordering vs. the
// previous module-load `socket.connect()`). With auth on and no session, the
// gated `/ws` upgrade 401s; a blind boot connect would just burn the reconnect
// loop down to "failed" before the user ever logs in. So we first ask `/api/me`
// who we are:
//   - auth disabled OR a valid session → set the phase, THEN connect (today's UX
//     for the disabled case; an authed user lands straight in the app). The
//     connect is synchronous within this resolution, so it still happens exactly
//     once and is in flight before React renders the shell — the app shell is
//     gated behind the non-"checking" phase, so the focused TerminalPane never
//     mounts (and never issues its first `subscribe`, which would be dropped on a
//     non-OPEN socket) until after this connect is issued.
//   - 401 → "anonymous"; the login screen renders and NO socket connect happens
//     until a successful login.
//   - the fetch itself REJECTS (server down/restarting, including an auth-OFF
//     deployment mid-flip) → "unreachable", and we auto-retry with capped
//     backoff. We deliberately do NOT fall back to "anonymous" here: a login
//     form whose submit would also network-fail is a worse UX than an honest
//     "can't reach the server, retrying" state, and an auth-OFF deployment has
//     no login at all. A retry that resolves proceeds exactly like a fresh boot
//     (disabled/authed → connect; 401 → anonymous), so the user never has to
//     reload by hand.
//
// The retry loop is a module-level timer (the popstate/socket precedent), NOT a
// React effect — `bootAuth` reschedules itself on each failure and clears the
// timer the moment a probe resolves. `retryAttempt` counts failures so the
// backoff schedule (see `unreachableRetryDelay`) advances; a successful probe
// resets it so a later outage starts the cadence over.
let bootRetryTimer: ReturnType<typeof setTimeout> | null = null
let bootRetryAttempt = 0

async function bootAuth(): Promise<void> {
  try {
    const resp = await fetch("/api/me", { credentials: "same-origin" })
    let body: MeBody | null = null
    if (resp.status === 200) {
      body = (await resp.json().catch(() => null)) as MeBody | null
    }
    // A probe resolved: cancel any pending retry and reset the cadence.
    if (bootRetryTimer !== null) {
      clearTimeout(bootRetryTimer)
      bootRetryTimer = null
    }
    bootRetryAttempt = 0
    const { phase, username } = phaseFromMe(resp.status, body)
    setState({ auth: { phase, username, error: null, pending: false } })
    if (phase === "disabled" || phase === "authed") {
      socket.connect()
    }
  } catch {
    // Network failure: show the honest unreachable state and schedule a retry.
    setState({
      auth: { phase: "unreachable", username: null, error: null, pending: false },
    })
    scheduleBootRetry()
  }
}

// Arm the next auto-retry of `bootAuth` after the capped-backoff delay for the
// current attempt count. Guarded so overlapping triggers (e.g. a manual "Retry
// now" while a timer is already armed) don't stack timers.
function scheduleBootRetry(): void {
  if (bootRetryTimer !== null) return
  const delay = unreachableRetryDelay(bootRetryAttempt)
  bootRetryAttempt += 1
  bootRetryTimer = setTimeout(() => {
    bootRetryTimer = null
    void bootAuth()
  }, delay)
}

// Manual "Retry now" from the unreachable screen. Cancel the pending backoff
// timer and probe immediately; `bootAuth` reschedules if it fails again.
export function retryBoot(): void {
  if (bootRetryTimer !== null) {
    clearTimeout(bootRetryTimer)
    bootRetryTimer = null
  }
  void bootAuth()
}

void bootAuth()

// Attempt a login. On success the phase flips to "authed" and the socket
// connects (the one connect for the auth-on path). A 401 surfaces the generic
// invalid-credentials message; a 429 surfaces a throttle message naming the
// Retry-After window; a network error surfaces a reachability message. The
// `pending` flag drives the submit button while the request is in flight.
export async function login(username: string, password: string): Promise<void> {
  setState({ auth: { ...state.auth, error: null, pending: true } })
  try {
    const resp = await fetch("/api/login", {
      method: "POST",
      credentials: "same-origin",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ username, password }),
    })
    if (resp.status === 200) {
      const body = (await resp.json().catch(() => null)) as MeBody | null
      const name =
        body && typeof body.username === "string" ? body.username : username
      setState({
        auth: { phase: "authed", username: name, error: null, pending: false },
      })
      socket.connect()
      return
    }
    const retryAfter =
      resp.status === 429
        ? parseRetryAfter(resp.headers.get("retry-after"))
        : undefined
    setState({
      auth: {
        ...state.auth,
        error: loginErrorMessage(resp.status, retryAfter),
        pending: false,
      },
    })
  } catch {
    setState({
      auth: { ...state.auth, error: LOGIN_NETWORK_MESSAGE, pending: false },
    })
  }
}

// The patch that wipes every piece of session-scoped state back to its initial
// value. Logout MUST apply this so the previous user's data does not leak into
// the next login's connect window: the app shell is unmounted while anonymous,
// but the store is a module-level singleton that survives a logout/login cycle,
// so a stale ViewModel, selection, draft, or open dialog would otherwise flash
// (or worse, expose another user's data) the instant the next session connects
// and before the first fresh ViewModel arrives. Auth is reset separately by the
// caller; everything below is the session-scoped surface. `pendingOrders` is
// folded in explicitly here so it is never left dangling either.
function clearSessionScopedState(): Partial<DuxState> {
  return {
    viewModel: null,
    selectedTarget: null,
    selectedSessionId: null,
    editorTarget: null,
    commitTarget: null,
    commitDraft: "",
    statusLine: { tone: "info", message: "" },
    // Every dialog/modal target, reset to closed.
    deleteTarget: null,
    deleteTerminalTarget: null,
    discardTarget: null,
    renameTarget: null,
    renameDraft: "",
    changeProviderTarget: null,
    projectInfoTarget: null,
    projectSettingsTarget: null,
    globalEnvOpen: false,
    attachWorktreeTarget: null,
    attachWorktreeEntries: [],
    attachWorktreeLoading: false,
    checkoutDefaultBranchTarget: null,
    removeProjectTarget: null,
    addProjectOpen: false,
    projectPathInspection: null,
    // The new-agent / fork / from-PR name dialog group.
    createAgentTarget: null,
    createAgentDraft: "",
    createAgentRandomize: false,
    createAgentGeneratedName: null,
    createAgentNamePending: false,
    createAgentPrInput: "",
    paletteOpen: false,
    macrosDialogOpen: false,
    macrosDraft: [],
    mobileScreen: "home",
    // Optimistic reorder overlays — explicitly cleared (was incidental before).
    pendingSessionOrder: null,
    pendingProjectOrder: null,
    // Any in-flight create-focus intent dies with the session, so a create
    // submitted before logout never yanks focus into the next login's workspace.
    pendingCreateFocus: null,
  }
}

// Log out: tell the server to destroy the session, deliberately disconnect the
// socket (suppressing the reconnect loop — `socket.close()` sets the
// closed-by-user flag), wipe all session-scoped state (so nothing from this user
// leaks into the next login — see `clearSessionScopedState`), and drop to the
// login screen. The server call is best-effort: even if it fails we still tear
// down the client-side session view, since the cookie is HttpOnly and the gate
// re-checks every request anyway.
export async function logout(): Promise<void> {
  socket.close()
  try {
    await fetch("/api/logout", { method: "POST", credentials: "same-origin" })
  } catch {
    // Ignore — the local teardown below is what matters for the UI.
  }
  setState({
    ...clearSessionScopedState(),
    auth: { phase: "anonymous", username: null, error: null, pending: false },
  })
}

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

// Select an agent session as the streamed target. Signature kept stable so
// existing callers continue to work unchanged.
export function selectSession(id: string | null): void {
  if (id === null) {
    // Clear the target FIRST so any synchronous re-render shows the fallback,
    // THEN collapse the mobile spoke so the back stack matches the screen. This
    // is the out-of-band clear path (e.g. an agent exit) — see
    // `unwindMobileSpoke`. Desktop stays on "home", so the unwind no-ops there.
    setState({ selectedTarget: null, selectedSessionId: null })
    // Tell the server to stop watching any worktree (a null id clears the global
    // changed-files watch). Without this the poller keeps reading the previously
    // selected session's worktree.
    socket.sendCommand("watch_changed_files", { session_id: null })
    unwindMobileSpoke()
    return
  }
  setState({
    selectedTarget: { kind: "agent", sessionId: id },
    selectedSessionId: id,
  })
  // Point the server-side changed-files watch at this session's worktree. The
  // engine's changed-files state is global and only set by whoever asks, so the
  // web must send this on selection (otherwise the pane stays empty — the TUI
  // was the only thing that ever set it).
  socket.sendCommand("watch_changed_files", { session_id: id })
}

// Select one of a session's companion terminals as the streamed target. The
// owning session id is retained so session-scoped UI keeps resolving.
export function selectTerminal(terminalId: string, sessionId: string): void {
  setState({
    selectedTarget: { kind: "terminal", terminalId, sessionId },
    selectedSessionId: sessionId,
  })
  // The watched worktree is the SESSION's, so watch the parent session even when
  // a companion terminal is the streamed target.
  socket.sendCommand("watch_changed_files", { session_id: sessionId })
}

// Ask the server to spawn a new companion terminal for a session. The server
// replies with `terminal_created`, which auto-focuses it via `onTerminalCreated`.
export function createTerminal(sessionId: string): void {
  socket.createTerminal(sessionId)
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

// Ask the server to close (delete) a companion terminal. It is removed from the
// ViewModel; if it was the focused target, the selection clears via the
// ViewModel-prune in `onViewModel`.
export function deleteTerminal(terminalId: string): void {
  socket.sendCommand("delete_terminal", { terminal_id: terminalId })
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

export function generateCommitMessage(sessionId: string): void {
  socket.sendCommand("generate_commit_message", { session_id: sessionId })
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
  socket.sendCommand("delete_session", {
    session_id: sessionId,
    delete_worktree: deleteWorktree,
  })
}

// Open the rename dialog for a session, pre-filling the current custom title
// (empty when none, so the placeholder shows the branch name).
export function openRename(sessionId: string): void {
  const session = state.viewModel?.sessions.find((s) => s.id === sessionId)
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
// back to the branch name; a non-empty title is validated server-side.
export function renameSession(sessionId: string, title: string): void {
  socket.sendCommand("rename_session", { session_id: sessionId, title })
}

// Submit the rename dialog and close it.
export function submitRename(): void {
  const id = state.renameTarget
  if (!id) return
  renameSession(id, state.renameDraft.trim())
  closeRename()
}

// Open the change-provider dialog for a session. The dialog pre-selects the
// session's current provider from the ViewModel.
export function openChangeProvider(sessionId: string): void {
  setState({ changeProviderTarget: sessionId })
}

export function closeChangeProvider(): void {
  setState({ changeProviderTarget: null })
}

// Ask the server to swap which provider a session uses. The server validates
// the provider against the configured list, persists it for the next launch,
// and reports the outcome (swapped / already-uses-it / still-running) on the
// status stream — nothing to do here but fire the command.
export function changeAgentProvider(sessionId: string, provider: string): void {
  socket.sendCommand("change_agent_provider", {
    session_id: sessionId,
    provider,
  })
}

// Ask the server to reconnect (relaunch) an agent. `force` starts a fresh
// session with no resume args (the TUI's force-reconnect); the default resumes
// the prior conversation when the provider supports it. Focus the session and
// bump `terminalEpoch` so the pane remounts and re-subscribes — the reconnect
// swaps in a new server-side provider, and the previously-attached forwarder is
// dead, so even an already-focused pane must re-issue `subscribe`. The server
// defers that subscribe until the freshly launched provider comes up.
export function reconnectSession(sessionId: string, force: boolean): void {
  socket.sendCommand("reconnect_session", { session_id: sessionId, force })
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
  socket.sendCommand("persist_global_env", { env })
}

export function openProjectSettings(projectId: string): void {
  setState({ projectSettingsTarget: projectId })
}

export function closeProjectSettings(): void {
  setState({ projectSettingsTarget: null })
}

export function openProjectInfo(projectId: string): void {
  setState({ projectInfoTarget: projectId })
}

export function closeProjectInfo(): void {
  setState({ projectInfoTarget: null })
}

export function openAddProject(): void {
  setState({ addProjectOpen: true, browseLoading: true, browseEntries: [] })
  socket.browseDir(null) // start at $HOME
}

export function closeAddProject(): void {
  setState({ addProjectOpen: false, projectPathInspection: null })
}

export function browseDir(path: string | null): void {
  // Navigating away abandons any pending/resolved branch inspection so a late
  // reply for the old selection can't resurface in the new directory.
  setState({ browseLoading: true, projectPathInspection: null })
  socket.browseDir(path)
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
  socket.inspectProjectPath(path)
}

// Drop any pending/resolved inspection (e.g. the user deselected the repo).
export function clearProjectInspection(): void {
  setState({ projectPathInspection: null })
}

export function addProject(path: string, name: string): void {
  socket.sendCommand("add_project", { path, name })
}

// Check out the repo's default branch first, then add it — the TUI's
// "Check Out & Add" path. Only offered for the Known warning (the server
// re-validates and rejects otherwise). The switch + add run server-side through
// the worker chain; the status stream reports the outcome.
export function addProjectCheckoutDefault(path: string, name: string): void {
  socket.sendCommand("add_project_checkout_default", { path, name })
}

export function openRemoveProject(projectId: string): void {
  setState({ removeProjectTarget: projectId })
}

export function closeRemoveProject(): void {
  setState({ removeProjectTarget: null })
}

export function removeProject(projectId: string): void {
  socket.sendCommand("remove_project", { project_id: projectId })
}

// Refresh a project's source checkout from remote (the TUI's
// `refresh_selected_project`). The server resolves the project, runs the pull
// against its source checkout, and reports busy/success/failure on the status
// stream — nothing to do here but fire the command.
export function pullProject(projectId: string): void {
  git
    .pullProject(projectId)
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
  git
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
  socket.listProjectWorktrees(projectId)
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
  socket.sendCommand("create_agent_from_worktree", {
    project_id: projectId,
    worktree_path: worktreePath,
    name,
  })
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
function openNameDialog(target: CreateAgentTarget): void {
  const randomize =
    target.kind !== "pr" &&
    (state.viewModel?.randomize_agent_names_by_default ?? false)
  setState({
    createAgentTarget: target,
    createAgentDraft: "",
    createAgentRandomize: randomize,
    createAgentGeneratedName: null,
    createAgentNamePending: randomize,
    createAgentPrInput: "",
  })
  if (randomize) socket.generateAgentName()
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
    socket.generateAgentName()
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

// Ask the server to create a new agent in a project. An empty name lets the
// server auto-generate a branch name (the equivalent outcome to the TUI's
// generate-a-pet-name path). With the checkbox checked the input is effectively
// never empty, so the empty path is the unchecked-and-blank case.
export function createAgent(projectId: string, name: string): void {
  socket.sendCommand("create_agent", { project_id: projectId, name })
}

// Ask the server to fork an existing session into a fresh branched worktree.
// Unlike create, a fork requires a non-empty name (the server rejects empty).
export function forkAgent(sessionId: string, name: string): void {
  socket.sendCommand("fork_session", { session_id: sessionId, name })
}

// Ask the server to create an agent checked out on a GitHub PR's head branch.
// `pr` is the raw reference (URL, `#123`, or `123`); the server resolves it via
// `gh pr view`. An empty `name` falls back to the PR head branch, matching the
// TUI prompt default. The lookup+create runs asynchronously: the command returns
// a busy status synchronously and the outcome arrives on the status stream.
export function createAgentFromPr(projectId: string, pr: string, name: string): void {
  socket.sendCommand("create_agent_from_pr", {
    project_id: projectId,
    pr,
    name,
  })
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
    const projectId = state.viewModel?.sessions.find(
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
// overlay clears when the next ViewModel confirms the order (or on error).
export function reorderSessions(projectId: string, orderedIds: string[]): void {
  setState({ pendingSessionOrder: { projectId, ids: orderedIds } })
  socket.sendCommand("reorder_sessions", {
    project_id: projectId,
    session_ids: orderedIds,
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
// ViewModel echo arrives within tens of milliseconds, so the brief reflow is
// acceptable and keeps the single-project drag overlay invariant untouched.
// Projects with fewer than two sessions are skipped — sorting them is a no-op
// that would only churn the wire.
export function sortAgents(by: SortKey): void {
  const sessions = state.viewModel?.sessions ?? []
  const projects = state.viewModel?.projects ?? []
  // A sort supersedes any in-flight drag: drop its overlay up front, or a
  // superseded drag order would linger on screen until something else clears
  // it (the overlay only retires on match/error/disconnect).
  setState(clearPendingOrders())
  for (const project of projects) {
    const projectSessions = sessions.filter((s) => s.project_id === project.id)
    if (projectSessions.length < 2) continue
    const orderedIds = sortedSessionIds(projectSessions, by)
    socket.sendCommand("reorder_sessions", {
      project_id: project.id,
      session_ids: orderedIds,
    })
  }
}

// Optimistically reorder the projects, then tell the server. `orderedIds` MUST
// be the complete ordered set of ALL project ids (both with and without agents);
// the server validates it as a strict permutation. The overlay clears when the
// next ViewModel confirms the order (or on error).
export function reorderProjects(orderedIds: string[]): void {
  setState({ pendingProjectOrder: orderedIds })
  socket.sendCommand("reorder_projects", { project_ids: orderedIds })
}

export function setPaletteOpen(open: boolean): void {
  setState({ paletteOpen: open })
}

// Run a macro by name against a target (an agent session id or a companion
// terminal id). The engine resolves the macro's text, enforces the surface gate,
// applies the newline transform, and writes to the target's PTY — the web only
// names the target + macro. The verbose `Sent macro "<name>".` confirmation
// rides the existing status lane (toast), so there is nothing to do here.
export function runMacro(targetId: string, name: string): void {
  socket.sendCommand("run_macro", { target_id: targetId, name })
}

// Open the macro-editor dialog, seeding the draft from the current ViewModel
// macros (a fresh copy so edits don't mutate the shared model). Runs in the
// click/palette handler that opens the dialog — never an effect.
export function openMacrosDialog(): void {
  const macros = state.viewModel?.macros ?? []
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
// on the status lane; a config reload refreshes `viewModel.macros`. The dialog
// closes optimistically — a rejection surfaces as an error toast, and reopening
// re-seeds from the (unchanged) ViewModel.
export function saveMacros(macros: MacroView[]): void {
  socket.sendCommand("update_macros", { entries: macros })
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
  socket.reconnect()
}

// Update the expanded sidebar width during a drag. Pass `persist` on release to
// write the final value to localStorage.
export function setSidebarWidth(width: string, persist = false): void {
  setState({ sidebarWidth: width })
  if (persist) {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, width)
  }
}
