import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { DuxSocket } from "./ws"
import type { ConnState, ViewModel } from "./types"

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
  paletteOpen: boolean
  sidebarWidth: string
}

// The expanded sidebar width is drag-resizable and persisted across reloads.
const SIDEBAR_WIDTH_KEY = "dux:sidebar-width"
const DEFAULT_SIDEBAR_WIDTH = "16rem"

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
  paletteOpen: false,
  sidebarWidth: loadSidebarWidth(),
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
}

socket.onConn = (conn) => {
  setState({ conn })
}

socket.onCommandResult = (status, error) => {
  if (error) {
    setState({ lastMessage: error })
    toast.error(error)
  } else if (status) {
    setState({ lastMessage: status.message })
    toast.success(status.message)
  }
}

socket.onError = (message) => {
  setState({ lastMessage: message })
  toast.error(message)
}

// A freshly created terminal auto-focuses so the user lands on it immediately.
socket.onTerminalCreated = (sessionId, terminalId) => {
  selectTerminal(terminalId, sessionId)
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

export function openCommit(sessionId: string): void {
  setState({ commitTarget: sessionId })
}

export function closeCommit(): void {
  setState({ commitTarget: null })
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
