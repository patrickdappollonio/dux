import { isSlotTabTarget, isTabGone } from "@/lib/agentTabs"
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
  /// The agent's slot tab as the spine names it, absent only while the spine
  /// has not arrived. Slot-ness is decided against this, never the session id.
  slotTabId?: string
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
    slotTabId,
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
    // Adopt before notifying: a heal replay must parse at the PTY's own grid, and
    // the handshake's replay was drawn for it, which the owner honours too.
    resize.noteRemoteGrid(grid, fromHandshake)
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

  // Extra tabs only: the slot tab's disappearance is its session's. Slot-ness is
  // asked of the same helper and `slotTabId` the socket URL was built from, or a
  // not-yet-arrived tab list reads as "gone" and stops the slot tab reconnecting.
  if (
    kind === "agent" &&
    (sessionId === null || !isSlotTabTarget(sessionId, id, slotTabId))
  ) {
    pty.shouldRetry = () => !isTabGone(live.current.sessionTabs ?? [], id)
    pty.onGone = () => handleTabGone(id)
  }

  pty.onBeat = (sequence) => beat.noteAnswer(sequence)
}
