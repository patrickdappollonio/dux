import { useSyncExternalStore } from "react"
import { toast } from "sonner"

import { DuxSocket } from "./ws"
import type { ConnState, ViewModel } from "./types"

// A tiny external store backed by `useSyncExternalStore`. A single module-level
// `DuxSocket` instance feeds it: ViewModel updates, connection state, and
// command/error results (surfaced as `lastMessage`). The PTY byte stream is
// NOT kept in React state — the terminal attaches to `socket.onPtyBytes`
// directly.

export interface DuxState {
  viewModel: ViewModel | null
  conn: ConnState
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

socket.connect()

export function useDux(): DuxState {
  return useSyncExternalStore(subscribe, getSnapshot)
}

export function selectSession(id: string | null): void {
  setState({ selectedSessionId: id })
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
