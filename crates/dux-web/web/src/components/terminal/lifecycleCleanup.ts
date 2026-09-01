import type { RefObject } from "react"
import type { FitAddon } from "@xterm/addon-fit"
import type { Terminal } from "@xterm/xterm"

import type { Heartbeat } from "@/lib/heartbeat"
import {
  getActivePtySocket,
  setActivePtySocket,
  type PtySocket,
} from "@/lib/ptySocket"

import type { ConnectionIdentity, TakeoverIntent } from "./channels"
import type { LinkPress } from "./linkPress"
import type { ResizeCoordinator } from "./resizeCoordinator"
import { retireConnectionIdentity } from "./socketCallbacks"
import { disposeTerminalSetup, type TerminalSetup } from "./terminalSetup"

type Disposable = { dispose: () => void }

type TerminalLifecycleCleanupOptions = {
  resize: ResizeCoordinator
  takeoverIntent: TakeoverIntent
  viewerRegridRef: RefObject<(() => void) | null>
  ownerRefitRef: RefObject<(() => void) | null>
  beat: Heartbeat
  beatRef: RefObject<Heartbeat | null>
  unregisterLifecycle: () => void
  unsubscribeRunProbe: () => void
  links: LinkPress
  inputWiring: Disposable
  touchWiring: Disposable
  noteVisibility: () => void
  localGridSubscription: Disposable
  connId: ConnectionIdentity
  pty: PtySocket
  ptyRef: RefObject<PtySocket | null>
  termRef: RefObject<Terminal | null>
  fitAddonRef: RefObject<FitAddon | null>
  terminalSetup: TerminalSetup
}

export function disposeTerminalLifecycle(
  options: TerminalLifecycleCleanupOptions,
): void {
  const {
    resize,
    takeoverIntent,
    viewerRegridRef,
    ownerRefitRef,
    beat,
    beatRef,
    unregisterLifecycle,
    unsubscribeRunProbe,
    links,
    inputWiring,
    touchWiring,
    noteVisibility,
    localGridSubscription,
    connId,
    pty,
    ptyRef,
    termRef,
    fitAddonRef,
    terminalSetup,
  } = options

  resize.dispose()
  takeoverIntent.clear()
  if (viewerRegridRef.current !== null) viewerRegridRef.current = null
  if (ownerRefitRef.current !== null) ownerRefitRef.current = null
  beat.stop()
  if (beatRef.current === beat) beatRef.current = null
  unregisterLifecycle()
  unsubscribeRunProbe()
  links.dispose()
  inputWiring.dispose()
  touchWiring.dispose()
  document.removeEventListener("visibilitychange", noteVisibility)
  localGridSubscription.dispose()
  retireConnectionIdentity(connId)
  pty.dispose()
  if (ptyRef.current === pty) ptyRef.current = null
  if (getActivePtySocket() === pty) setActivePtySocket(null)
  termRef.current = null
  fitAddonRef.current = null
  disposeTerminalSetup(terminalSetup)
}
