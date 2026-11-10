import { agentTabSocketGone } from "@/lib/agentTabs"
import { terminalSocketGone } from "@/lib/terminals"
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

  // Every agent tab, the slot's included: a clean exit of the slot tab hands the
  // slot to a sibling and deletes the exited row, so this socket's own tab can
  // vanish while its session stays up. Keyed on the LIVE tab list rather than on
  // slot-ness, which is what keeps a not-yet-arrived spine from reading as gone.
  if (kind === "agent" && sessionId !== null) {
    pty.shouldRetry = () =>
      !agentTabSocketGone(live.current.sessionTabs, sessionId, id)
    pty.onGone = () => handleTabGone(id)
  }

  // A terminal is runtime-only, so one that is not on the spine is gone for good
  // and its route keeps 404ing. `onGone` is left at its quiet default: the spine
  // update that removed the terminal also prunes its selection and unmounts the
  // pane, so there is nothing for the socket to announce.
  if (kind === "terminal") {
    pty.shouldRetry = () => !terminalSocketGone(live.current.spineTerminals, id)
  }

  pty.onBeat = (sequence) => beat.noteAnswer(sequence)
}
