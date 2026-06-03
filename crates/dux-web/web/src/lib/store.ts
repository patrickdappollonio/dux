import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { DuxSocket } from "./ws"
import type { ConnState, DirEntryView, FileDiff, ViewModel } from "./types"

// The currently-streamed target: either an agent session or one of its
// companion terminals. Both carry a `sessionId` so session-scoped UI (the
// breadcrumb, changed files) keeps working regardless of which is focused.
export type SelectedTarget =
  | { kind: "agent"; sessionId: string }
  | { kind: "terminal"; terminalId: string; sessionId: string }

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
  lastMessage: string
  commitTarget: string | null
  commitDraft: string
  deleteTarget: string | null
  globalEnvOpen: boolean
  projectSettingsTarget: string | null
  addProjectOpen: boolean
  browsePath: string
  browseEntries: DirEntryView[]
  browseLoading: boolean
  removeProjectTarget: string | null
  createAgentTarget: string | null
  paletteOpen: boolean
  sidebarWidth: string
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

let state: DuxState = {
  viewModel: null,
  conn: "connecting",
  selectedTarget: null,
  selectedSessionId: null,
  lastMessage: "",
  commitTarget: null,
  commitDraft: "",
  deleteTarget: null,
  globalEnvOpen: false,
  projectSettingsTarget: null,
  addProjectOpen: false,
  browsePath: "",
  browseEntries: [],
  browseLoading: false,
  removeProjectTarget: null,
  createAgentTarget: null,
  paletteOpen: false,
  sidebarWidth: loadSidebarWidth(),
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
  setState({ viewModel: vm })
  // If the focused target vanished (an agent session was removed, or a
  // companion terminal exited and was pruned server-side), drop the selection
  // so the center pane shows the empty state instead of a dead terminal.
  pruneSelectionIfGone(vm)
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
    selectSession(null)
  }
}

socket.onConn = (conn) => {
  setState({ conn })
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
    setState({ lastMessage: error })
    toast.error(error)
  } else if (status) {
    setState({ lastMessage: status.message })
    toastForTone(status.tone, status.message)
  }
}

socket.onError = (message) => {
  setState({ lastMessage: message })
  toast.error(message)
}

// Asynchronous status/lifecycle events (background push/pull completing, an
// agent launch failing, a PTY exiting). Surface them as a toast toned by the
// engine's StatusTone and keep the latest in the status bar.
socket.onStatus = (tone, message) => {
  setState({ lastMessage: message })
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

socket.connect()

export function useDux(): DuxState {
  return useSyncExternalStore(subscribe, getSnapshot)
}

// Select an agent session as the streamed target. Signature kept stable so
// existing callers continue to work unchanged.
export function selectSession(id: string | null): void {
  if (id === null) {
    setState({ selectedTarget: null, selectedSessionId: null })
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

// Ask the server to close (delete) a companion terminal. It is removed from the
// ViewModel; if it was the focused target, the selection clears via the
// ViewModel-prune in `onViewModel`.
export function deleteTerminal(terminalId: string): void {
  socket.sendCommand("delete_terminal", { terminal_id: terminalId })
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

export function openCreateAgent(projectId: string): void {
  setState({ createAgentTarget: projectId })
}

export function closeCreateAgent(): void {
  setState({ createAgentTarget: null })
}

// Ask the server to create a new agent in a project. An empty name lets the
// server auto-generate a branch name.
export function createAgent(projectId: string, name: string): void {
  socket.sendCommand("create_agent", { project_id: projectId, name })
}

export function setPaletteOpen(open: boolean): void {
  setState({ paletteOpen: open })
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
