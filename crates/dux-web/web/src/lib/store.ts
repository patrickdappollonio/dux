import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { sanitizeAgentName } from "./agentName"
import { ordersMatch } from "./reorder"
import { DuxSocket } from "./ws"
import type { ConnState, DirEntryView, FileDiff, ViewModel } from "./types"

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
// command/error results (surfaced as `lastMessage`). The PTY byte stream is
// NOT kept in React state — the terminal attaches to `socket.onPtyBytes`
// directly.

export interface DuxState {
  viewModel: ViewModel | null
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
  lastMessage: string
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
  addProjectOpen: boolean
  browsePath: string
  browseEntries: DirEntryView[]
  browseLoading: boolean
  removeProjectTarget: string | null
  // The project pending a default-branch checkout confirmation, or null. The
  // checkout moves the source checkout's HEAD, so the web confirms first (the
  // TUI runs it straight from a deliberate palette/keybinding action).
  checkoutDefaultBranchTarget: string | null
  // The name-input dialog target: a fresh agent in a project, a fork of an
  // existing session, or null (closed). One dialog component switches on `kind`.
  createAgentTarget: CreateAgentTarget | null
  // The session pending a rename, or null. The dialog pre-fills the current
  // title (or empty, so the placeholder shows the branch name).
  renameTarget: string | null
  renameDraft: string
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
  //   - `createAgentNamePending`: a generate-name request is in flight. Drives
  //     the dialog's spinner and disables the input so a late reply can never
  //     clobber text the user typed in the meantime. Explicit rather than
  //     inferred from an empty draft, so manually clearing the input doesn't
  //     fake a phantom "generating" state.
  createAgentNamePending: boolean
  paletteOpen: boolean
  // Which screen the mobile shell is showing. Always "home" on desktop, which
  // ignores it. Only the mobile UI advances it past "home".
  mobileScreen: MobileScreen
  // Optimistic drag-and-drop ordering overlays (see `applyPendingOrders`). Each
  // is set the moment a drag ends and cleared once the server's next ViewModel
  // confirms the new order (or an error status arrives). Null when no reorder is
  // in flight, which is the overwhelmingly common case.
  pendingSessionOrder: PendingSessionOrder | null
  pendingProjectOrder: string[] | null
  sidebarWidth: string
  // Whether the diff viewer shows the old/new line-number gutters. Mirrors the
  // TUI's `toggle-diff-line-numbers` (config `ui.show_diff_line_numbers`),
  // defaulting ON; persisted in localStorage like the sidebar width.
  showDiffLineNumbers: boolean
  currentDiff: {
    sessionId: string
    path: string
    diff: FileDiff | null
    error: string | null
    loading: boolean
  } | null
}

// The expanded sidebar width is drag-resizable and persisted across reloads.
// 18rem gives agent names breathing room next to the PR/status badges; a
// previously persisted width still wins.
const SIDEBAR_WIDTH_KEY = "dux:sidebar-width"
const DEFAULT_SIDEBAR_WIDTH = "18rem"

function loadSidebarWidth(): string {
  return localStorage.getItem(SIDEBAR_WIDTH_KEY) || DEFAULT_SIDEBAR_WIDTH
}

// Diff line-number gutters default ON (matching the TUI default and the prior
// web behavior). Only an explicit persisted "false" turns them off, so a
// missing key keeps the gutters visible.
const SHOW_DIFF_LINE_NUMBERS_KEY = "dux:show-diff-line-numbers"

function loadShowDiffLineNumbers(): boolean {
  return localStorage.getItem(SHOW_DIFF_LINE_NUMBERS_KEY) !== "false"
}

let state: DuxState = {
  viewModel: null,
  conn: "connecting",
  selectedTarget: null,
  selectedSessionId: null,
  terminalEpoch: 0,
  lastMessage: "",
  commitTarget: null,
  commitDraft: "",
  deleteTarget: null,
  deleteTerminalTarget: null,
  discardTarget: null,
  globalEnvOpen: false,
  projectSettingsTarget: null,
  addProjectOpen: false,
  browsePath: "",
  browseEntries: [],
  browseLoading: false,
  removeProjectTarget: null,
  checkoutDefaultBranchTarget: null,
  createAgentTarget: null,
  renameTarget: null,
  renameDraft: "",
  createAgentDraft: "",
  createAgentRandomize: false,
  createAgentGeneratedName: null,
  createAgentNamePending: false,
  paletteOpen: false,
  mobileScreen: "home",
  pendingSessionOrder: null,
  pendingProjectOrder: null,
  sidebarWidth: loadSidebarWidth(),
  showDiffLineNumbers: loadShowDiffLineNumbers(),
  currentDiff: null,
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

function getSnapshot(): DuxState {
  return state
}

// The single socket instance for the whole app. Exported so components that
// talk to the PTY (terminal) or issue commands (palette) can use it directly.
export const socket = new DuxSocket(`ws://${location.host}/ws`)

socket.onViewModel = (vm) => {
  setState({
    viewModel: vm,
    // Retire each optimistic order overlay once the server's order matches it;
    // until then keep showing the overlay so the row doesn't snap back during
    // the round-trip. A stale (non-matching) overlay is kept — a later ViewModel
    // confirming our reorder will clear it; an error status clears it outright.
    pendingSessionOrder: reconcilePendingSessionOrder(vm, state.pendingSessionOrder),
    pendingProjectOrder: reconcilePendingProjectOrder(vm, state.pendingProjectOrder),
  })
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

socket.onConn = (conn) => {
  // A connection break invalidates any in-flight optimistic reorder: the
  // command (or its rejection) may have been lost, and after the reconnect
  // nothing would ever reconcile a non-matching overlay — leaving the UI
  // showing an order the server never persisted. Snap back to authoritative.
  const patch = conn === "closed" || conn === "failed" ? clearPendingOrders() : {}
  setState({ conn, ...patch })
}

// Show a toast colored by the engine's StatusTone. dux uses Info for positive
// confirmations ("X succeeded"), so Info maps to a success toast; Busy is a
// neutral in-progress notice; Warning/Error map directly. Shared by the
// synchronous command-result path and the async status stream so a "busy"
// result is never shown as a green success.
function toastForTone(tone: string, message: string): void {
  switch (tone) {
    case "error":
      toast.error(message)
      break
    case "warning":
      toast.warning(message)
      break
    case "busy":
      toast.info(message)
      break
    default:
      // "info" (and any unknown tone) is a positive confirmation.
      toast.success(message)
      break
  }
}

socket.onCommandResult = (status, error) => {
  if (error) {
    // A rejected reorder (stale/partial id set) comes back as an error here;
    // drop any optimistic overlay so the UI reverts to the server's order.
    setState({ lastMessage: error, ...clearPendingOrders() })
    toast.error(error)
  } else if (status) {
    setState({ lastMessage: status.message })
    toastForTone(status.tone, status.message)
  }
}

socket.onError = (message) => {
  setState({ lastMessage: message, ...clearPendingOrders() })
  toast.error(message)
}

// Reset both optimistic order overlays. Returned as a patch so callers can fold
// it into a single `setState`. Used on every error path so a rejected reorder
// snaps the UI back to the server's authoritative order.
function clearPendingOrders(): Partial<DuxState> {
  return { pendingSessionOrder: null, pendingProjectOrder: null }
}

// Asynchronous status/lifecycle events (background push/pull completing, an
// agent launch failing, a PTY exiting). Surface them as a toast toned by the
// engine's StatusTone and keep the latest in the status bar.
socket.onStatus = (tone, message) => {
  // An error-toned async status also unwinds any optimistic reorder overlay.
  const patch = tone === "error" ? clearPendingOrders() : {}
  setState({ lastMessage: message, ...patch })
  toastForTone(tone, message)
}

// A freshly created terminal auto-focuses so the user lands on it immediately.
socket.onTerminalCreated = (sessionId, terminalId) => {
  selectTerminal(terminalId, sessionId)
}

socket.onCommitMessage = (message) => {
  // Fill the open commit dialog's draft with the generated message.
  if (state.commitTarget !== null) {
    setState({ commitDraft: message })
  }
}

socket.onDiff = (sessionId, path, diff, error) => {
  // Ignore stale responses for a file the user already navigated away from.
  if (state.currentDiff?.path !== path || state.currentDiff?.sessionId !== sessionId) {
    return
  }
  setState({ currentDiff: { sessionId, path, diff, error, loading: false } })
}

socket.onDirEntries = (path, entries, error) => {
  setState({ browsePath: path, browseEntries: error ? [] : entries, browseLoading: false })
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

socket.connect()

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
    unwindMobileSpoke()
    return
  }
  setState({
    selectedTarget: { kind: "agent", sessionId: id },
    selectedSessionId: id,
  })
}

// Select one of a session's companion terminals as the streamed target. The
// owning session id is retained so session-scoped UI keeps resolving.
export function selectTerminal(terminalId: string, sessionId: string): void {
  setState({
    selectedTarget: { kind: "terminal", terminalId, sessionId },
    selectedSessionId: sessionId,
  })
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
  socket.sendCommand("discard_file", { session_id: sessionId, path })
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

export function requestDiff(sessionId: string, path: string): void {
  setState({ currentDiff: { sessionId, path, diff: null, error: null, loading: true } })
  socket.getDiff(sessionId, path)
}

export function closeDiff(): void {
  setState({ currentDiff: null })
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

export function openAddProject(): void {
  setState({ addProjectOpen: true, browseLoading: true, browseEntries: [] })
  socket.browseDir(null) // start at $HOME
}

export function closeAddProject(): void {
  setState({ addProjectOpen: false })
}

export function browseDir(path: string | null): void {
  setState({ browseLoading: true })
  socket.browseDir(path)
}

export function addProject(path: string, name: string): void {
  socket.sendCommand("add_project", { path, name })
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
  socket.sendCommand("pull_project", { project_id: projectId })
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
  socket.sendCommand("checkout_project_default_branch", { project_id: projectId })
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

// Shared opener for both modes of the name dialog. Pre-checks the randomize
// default and requests a name right away so the input previews it, exactly like
// the TUI prompt. Runs in the click handler that opens the dialog — never an
// effect — so there is no set-state-in-effect.
function openNameDialog(target: CreateAgentTarget): void {
  const randomize = state.viewModel?.randomize_agent_names_by_default ?? false
  setState({
    createAgentTarget: target,
    createAgentDraft: "",
    createAgentRandomize: randomize,
    createAgentGeneratedName: null,
    createAgentNamePending: randomize,
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
  })
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

// Submit the name dialog: dispatch create or fork based on the current target,
// then close. Mirrors the TUI, where the same name prompt drives both flows.
export function submitNameDialog(name: string): void {
  const target = state.createAgentTarget
  if (!target) return
  if (target.kind === "new") {
    createAgent(target.projectId, name)
  } else {
    forkAgent(target.sessionId, name)
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

// Flip the diff line-number gutters and persist the choice (the TUI's
// `toggle-diff-line-numbers`). Both the old and new gutters hide together.
export function toggleDiffLineNumbers(): void {
  const next = !state.showDiffLineNumbers
  setState({ showDiffLineNumbers: next })
  localStorage.setItem(SHOW_DIFF_LINE_NUMBERS_KEY, String(next))
}
