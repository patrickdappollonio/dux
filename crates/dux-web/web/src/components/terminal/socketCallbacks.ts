import { isTabGone } from "@/lib/agentTabs"
import type { Heartbeat } from "@/lib/heartbeat"
import type { HandshakeOwner } from "@/lib/ptyOwnership"
import type { PtySocket } from "@/lib/ptySocket"
import { handleTabGone, noteOwnPtyConnection } from "@/lib/store"
import type { ConnState } from "@/lib/types"

import type { AttachReplay } from "./attachReplay"
import type { ConnectionIdentity } from "./channels"
import type { LiveSettings } from "./liveValues"
import type { ResizeCoordinator } from "./resizeCoordinator"

type TerminalSocketCallbackOptions = {
  pty: PtySocket
  kind: "agent" | "terminal"
  id: string
  sessionId: string | null
  live: LiveSettings
  connId: ConnectionIdentity
  resize: ResizeCoordinator
  attach: AttachReplay
  beat: Heartbeat
  seedOwnershipFromConnected: (
    myConnId: string,
    owner: HandshakeOwner,
    ownerEpoch?: number,
    ownerDevice?: string,
  ) => void
  noteRemotePtyGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake: boolean,
  ) => void
  noteSocketOpen: () => void
  noteAttachEpoch: (epoch: number) => void
  notePtyConn: (state: ConnState) => void
  setReconnecting: (value: boolean) => void
  resetReplayWait: () => void
}

export function retireConnectionIdentity(
  connectionIdentity: ConnectionIdentity,
): boolean {
  const connectionId = connectionIdentity.read()
  if (connectionId === null) return false
  noteOwnPtyConnection(connectionId, false)
  connectionIdentity.write(null)
  return true
}

export function registerTerminalSocketCallbacks(
  options: TerminalSocketCallbackOptions,
): void {
  const {
    pty,
    kind,
    id,
    sessionId,
    live,
    connId,
    resize,
    attach,
    beat,
    seedOwnershipFromConnected,
    noteRemotePtyGrid,
    noteSocketOpen,
    noteAttachEpoch,
    notePtyConn,
    setReconnecting,
    resetReplayWait,
  } = options

  pty.onConnected = (connectionId, owner, ownerEpoch, ownerDevice) => {
    connId.write(connectionId)
    noteOwnPtyConnection(connectionId, true)
    seedOwnershipFromConnected(connectionId, owner, ownerEpoch, ownerDevice)
  }

  pty.onPtyGrid = (grid, fromHandshake) => {
    // Adopt before notifying: a heal replay must parse at the PTY's own grid.
    resize.noteRemoteGrid(grid)
    noteRemotePtyGrid(grid, fromHandshake)
  }

  pty.onOpen = () => {
    // Each open gets a new server-side identity. The stale one must not answer
    // ownership questions while the next handshake is still in flight.
    if (!retireConnectionIdentity(connId)) connId.write(null)
    setReconnecting(false)
    noteSocketOpen()

    const { firstOpen, epoch } = attach.noteOpen()
    resize.noteOpen(firstOpen)
    noteAttachEpoch(epoch)
    resetReplayWait()
    beat.reset()
    resize.resyncToForeground()
  }

  pty.onReconnecting = () => {
    setReconnecting(true)
    retireConnectionIdentity(connId)
  }

  pty.onConn = (connectionState) => {
    if (connectionState === "failed") setReconnecting(false)
    if (connectionState !== "open") beat.reset()
    notePtyConn(connectionState)
  }

  if (kind === "agent" && id !== sessionId) {
    pty.shouldRetry = () => !isTabGone(live.current.sessionTabs ?? [], id)
    pty.onGone = () => handleTabGone(id)
  }

  pty.onBeat = (sequence) => beat.noteAnswer(sequence)
}
