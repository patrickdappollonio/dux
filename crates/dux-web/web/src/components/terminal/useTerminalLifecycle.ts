// The one lifecycle owner for the terminal pane: it creates the terminal, opens
// the socket, wires every listener the pair needs, and tears all of it down.
//
// It re-runs only when the streamed target changes, so every closure it creates
// outlives the render that created it and nothing may reach one by being
// captured: read-only values come from the live-settings container, values the
// wiring writes come from named channels, and the rest is an explicit port.
//
// `ptyUrl` is derived here rather than handed in, because it is a function of
// the target and nothing else.
//
// Everything below registers inside the one effect and is disposed by the one
// cleanup. The pane's own separate registration effects are inventoried in
// `TerminalPane` beside their code.
import { useEffect, useRef, type RefObject } from "react"
import type { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"

import {
  PtySocket,
  agentPtyUrl,
  setActivePtySocket,
  tabPtyUrl,
  terminalSocketUrl,
} from "@/lib/ptySocket"
import { isSlotTabTarget } from "@/lib/agentTabs"
import { shouldSendViewed, visibleSinceAfterTransition } from "@/lib/viewedPing"
import { createHeartbeat, type Heartbeat } from "@/lib/heartbeat"
import { onServerRunUnconfirmed } from "@/lib/serverRun"
import { registerPageLifecycle } from "@/lib/pageLifecycle"
import { registerLayoutGestureHolder } from "@/lib/layoutGesture"
import type { ConnState } from "@/lib/types"
import type { TerminalOwnerRef } from "@/lib/store"
import { ownerSessionId } from "@/lib/terminalOwner"

import type { LiveSettings } from "./liveValues"
import type {
  ConnectionIdentity,
  ModifierLatch,
  OwnershipVerdict,
  TakeoverIntent,
} from "./channels"
import type { HandshakeOwner } from "@/lib/ptyOwnership"
import { createResizeCoordinator } from "./resizeCoordinator"
import { createAttachReplay } from "./attachReplay"
import { plainBounce } from "./plainBounce"
import { registerTerminalInputWiring } from "./inputWiring"
import { registerTerminalTouchWiring } from "./touchWiring"
import { createTerminalSetup, openTerminal } from "./terminalSetup"
import { registerTerminalSocketCallbacks } from "./socketCallbacks"
import { disposeTerminalLifecycle } from "./lifecycleCleanup"

/// The streamed target: an agent tab, or a companion terminal of either owner.
/// `id` is the FOCUSED TAB id for an agent and the terminal id for a terminal.
/// `slotTabId` is the agent's slot tab as the spine names it, absent only while
/// the spine has not arrived; slot-ness is decided against it, never against
/// the session id.
export type TerminalTarget =
  | { kind: "agent"; id: string; sessionId: string; slotTabId?: string }
  | { kind: "terminal"; id: string; owner: TerminalOwnerRef }

/// Everything the lifecycle needs that is neither a read-only setting nor one
/// of the three channels: the DOM it mounts into, the handles the rest of the
/// pane reads the live terminal and socket through, and the four calls back
/// into the component.
export type TerminalLifecyclePorts = {
  hostRef: RefObject<HTMLDivElement | null>
  containerRef: RefObject<HTMLDivElement | null>
  termRef: RefObject<Terminal | null>
  fitAddonRef: RefObject<FitAddon | null>
  ptyRef: RefObject<PtySocket | null>
  composeInputRef: RefObject<HTMLTextAreaElement | null>
  /// The pointer type of the most recent press on the host, written by the
  /// pane's own JSX handler and read by the contextmenu guard.
  pointerTypeRef: RefObject<string>
  /// The attention-grace transition trackers, shared with the ownership-gain
  /// effect outside this hook.
  visibleSinceRef: RefObject<number | undefined>
  prevVisibleRef: RefObject<boolean | undefined>
  /// The armed take-over, consumed by the ONE confirmed resize write below.
  takeoverIntent: TakeoverIntent
  /// Installed here over this mount's coordinator: the pane calls it to
  /// re-assert the adopted grid, so a viewer re-grid stays a coordinator act.
  viewerRegridRef: RefObject<(() => void) | null>
  /// Installed here too: the pane's relayout calls it when new cell metrics need
  /// a refit, so that fit obeys the coordinator's holds.
  ownerRefitRef: RefObject<(() => void) | null>
  /// Installed by the pane and read here: the coordinator's ResizeObserver calls
  /// it instead of fitting while a watcher renders faithfully.
  viewerRelayoutRef: RefObject<(() => void) | null>
  live: LiveSettings
  mods: ModifierLatch
  ownership: OwnershipVerdict
  connId: ConnectionIdentity
  /// The PTY socket's connection state. Owns the LOST state and the take-over
  /// intent's lifetime; see `ownership.ts`'s `notePtyConn`.
  notePtyConn: (state: ConnState) => void
  /// Re-seed the ownership verdict from the `connected` handshake. The frame
  /// lands here; the decision lives in the ownership machine. `ownerDevice` is
  /// the handshake's `owner_device`, a watcher's only source of a device name,
  /// since a mere attach hears no `pty.owner` broadcast.
  seedOwnershipFromConnected: (
    myConnId: string,
    owner: HandshakeOwner,
    ownerEpoch?: number,
    ownerDevice?: string,
  ) => void
  /// Record a grid the wire reported for this PTY (the `connected` handshake's
  /// snapshot, then every applied change). The viewer-grid machine decides what
  /// it means; the frames land here.
  noteRemotePtyGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake: boolean,
  ) => void
  /// Record THIS xterm's grid, so the pane can tell whether it is rendering at
  /// the geometry the child is drawing for.
  noteLocalGrid: (grid: { rows: number; cols: number }) => void
  /// The socket opened, which retires any heal bounce that was in flight.
  noteSocketOpen: () => void
  /// A new attach epoch was minted by this open. The pane resets its cover and
  /// its replay clock on it, and ignores any applied signal for an older one.
  noteAttachEpoch: (epoch: number) => void
  /// The replay for `epoch` has been PARSED, so the picture exists. This is what
  /// clears the cover; the socket merely opening never does.
  noteReplayApplied: (epoch: number) => void
  focusTypingSurface: () => void
  onClipboardPaste: (e: ClipboardEvent) => void
  /// Arm the force-text-paste hatch. The key handler here arms it and the
  /// pane's own paste listener consumes it, because a key event carries no
  /// clipboard contents and a paste event carries no modifiers.
  armForcedTextPaste: () => void
  setReconnecting: (value: boolean) => void
  /// The pane's replay-wait clock. Owned by the pane (the render reads whether it
  /// has expired) and RESET here on every attach epoch, because each open's
  /// patience starts from zero.
  resetReplayWait: () => void
  /// The rendered ownership verdict, read for one thing: the periodic frame's
  /// cadence depends on it, and a change must clear the armed timer.
  isOwnerRendered: boolean
}

export function useTerminalLifecycle(
  target: TerminalTarget,
  ports: TerminalLifecyclePorts,
): void {
  const { kind, id } = target
  // The owning session id, when there is one: the agent's own session, or a
  // session-owned terminal's parent. A PROJECT or STANDALONE terminal has none.
  const sessionId =
    target.kind === "agent" ? target.sessionId : ownerSessionId(target.owner)
  // For an agent, the slot tab uses the session PTY route and an extra tab its
  // own nested route; a terminal uses its owner's nested route. Slot-ness is
  // decided against the spine's `slotTabId`, never the session id. Both forms
  // address the slot tab's identical PTY; the session route is the one used
  // here, which keeps the slot tab out of the per-tab socket quota.
  const slotTabId = target.kind === "agent" ? target.slotTabId : undefined
  const ptyUrl =
    target.kind === "agent"
      ? isSlotTabTarget(target.sessionId, target.id, slotTabId)
        ? agentPtyUrl(target.sessionId)
        : tabPtyUrl(target.sessionId, target.id)
      : terminalSocketUrl(target.owner, target.id)

  const {
    hostRef,
    containerRef,
    termRef,
    fitAddonRef,
    ptyRef,
    composeInputRef,
    pointerTypeRef,
    visibleSinceRef,
    prevVisibleRef,
    takeoverIntent,
    viewerRegridRef,
    ownerRefitRef,
    viewerRelayoutRef,
    live,
    mods,
    ownership,
    connId,
    seedOwnershipFromConnected,
    noteRemotePtyGrid,
    noteLocalGrid,
    noteSocketOpen,
    noteAttachEpoch,
    noteReplayApplied,
    notePtyConn,
    focusTypingSurface,
    onClipboardPaste,
    armForcedTextPaste,
    setReconnecting,
    resetReplayWait,
    isOwnerRendered,
  } = ports

  // This mount's heartbeat, so the cadence can be retimed from outside the big
  // attach effect. Null whenever no socket is wired.
  const beatRef = useRef<Heartbeat | null>(null)
  // A change of ownership changes the beat's cadence, and the pending timer was
  // armed under the old one; re-arming keeps the engine's attention flag from
  // staying lit a whole slow period past the boundary.
  useEffect(() => {
    beatRef.current?.resync()
  }, [isOwnerRendered])

  useEffect(() => {
    const host = hostRef.current
    const container = containerRef.current
    if (!host || !container) return

    const terminalSetup = createTerminalSetup({ host, id, live })
    const { term, links, isMac } = terminalSetup
    // Opening the socket is the subscription: it launches the tab if it is not
    // running, with resume decided server-side. A dormant tab is never
    // auto-mounted, so reaching here for one is an intentional launch.
    // Registered as the active socket so the macro picker can write to it.
    const pty = new PtySocket(ptyUrl)
    ptyRef.current = pty
    setActivePtySocket(pty)
    // Constructed before the addon so the coordinator below can be given the
    // socket's own `sendResize`; `connect()` is still the explicit act below.
    const fit = new FitAddon()
    term.loadAddon(fit)
    // The coordinator is the one owner of `fit.fit()` and of every frame that
    // tells the child its size (see its module doc for the font exception).
    // This flag says whether the frame just written carried the take-over flag,
    // and is read by the coordinator immediately after the call.
    let lastSendFlagged = false
    const resize = createResizeCoordinator({
      term,
      fit,
      // The one place a take-over intent is consumed: every resize frame passes
      // through here, so whichever reaches the wire first carries the flag.
      //
      // Cleared only on a confirmed write, which is what `sendResize` answers:
      // a CONNECTING or CLOSED socket discards the frame silently, and clearing
      // on a discarded one spends the take-over on nothing.
      sendResize: (rows, cols) => {
        const takeover = takeoverIntent.read()
        // A self-succession names the ghost it expects to displace; a pressed
        // take-over names nobody, because a press may take from anyone. The
        // server refuses the transfer when the named ghost no longer holds the
        // pty, and this client then lands as a watcher with the card.
        const sent = takeover
          ? pty.sendResize(rows, cols, true, takeoverIntent.expectedOwner())
          : pty.sendResize(rows, cols)
        if (sent && takeover) takeoverIntent.clear()
        // The coordinator books a plain send's geometry immediately and a
        // flagged one's only once the pty reports it back: a claim can be
        // refused whole.
        lastSendFlagged = sent && takeover
        return sent
      },
      lastSendWasFlagged: () => lastSendFlagged,
      isOwner: () => ownership.read(),
      onViewerLayout: () => viewerRelayoutRef.current?.(),
    })
    // The pane's handle on this mount's grid adoption (see the port's doc).
    viewerRegridRef.current = () => resize.applyViewerGrid()
    ownerRefitRef.current = () => resize.refitForFonts()
    const localGridSubscription = openTerminal({
      setup: terminalSetup,
      container,
      fit,
      resize,
      termRef,
      fitAddonRef,
      noteLocalGrid,
    })

    const inputWiring = registerTerminalInputWiring({
      term,
      pty,
      container,
      isMac,
      live,
      mods,
      ownership,
      pointerTypeRef,
      replayInFlight: () => attach.replayInFlight(),
      focusTypingSurface,
      onClipboardPaste,
      armForcedTextPaste,
    })

    // The link-press machine owns the capture-phase intercept, the hover cache,
    // the opener and the activation counter the touch probe reads; its module
    // doc has the reasons for the phase and the press-time decision.
    links.attach(container)

    const touchWiring = registerTerminalTouchWiring({
      term,
      host,
      container,
      composeInputRef,
      live,
      ownership,
      resize,
      links,
    })

    // All the sizing plumbing is the coordinator's: xterm's resize event, the
    // mount fit, its dedupe seed, the no-first-frame fallback and the observer.
    //
    // It observes the host, not the container: the relayout's below-floor
    // overflow branch pins the container to the adopted grid's pixel size, and a
    // pinned box never moves with the window, so a watcher observing it could
    // never leave pan mode. The host's box is never pinned, and the owner's
    // container is `h-full w-full` of it, so the two resize together.
    resize.start(host)
    // The attach-and-replay machine owns the reopen's repaint: the generation
    // dedupe, the reset, the drain and the focus-report suppression window.
    const attach = createAttachReplay({
      term,
      replayGeneration: () => pty.replayGeneration,
      needsFirstFrameResize: resize.needsFirstFrameResize,
      firstFrameLanded: resize.firstFrameLanded,
    })
    pty.onBytes((bytes) => attach.onBytes(bytes))
    // An unconfirmed run retires the replay high-water mark because replay
    // generation counters restart with the server process.
    const unsubscribeRunProbe = onServerRunUnconfirmed(() => {
      attach.forgetAppliedGeneration()
    })
    // Clear the cover only after replay is parsed and applied, not at socket open.
    attach.onReplayApplied((epoch) => noteReplayApplied(epoch))
    // The one periodic client frame (see `lib/heartbeat.ts`): viewed semantics
    // and an application-level beat on one timer, never a second pinger. The
    // server's own ping is send-only, so it cannot see the half-open socket a
    // radio handoff leaves behind; a missed answer drops the socket and lets the
    // ordinary retry path reattach, plain.
    const beat = createHeartbeat({
      send: (n, viewed) => pty.sendBeat(n, viewed),
      isOwner: () => ownership.read(),
      viewed: () =>
        shouldSendViewed({
          isOwner: ownership.read(),
          visible: document.visibilityState === "visible",
          now: Date.now(),
          visibleSince: visibleSinceRef.current,
          graceMs: live.current.attentionGraceMs,
        }),
      onStalled: () => pty.dropForRetry(),
    })
    beatRef.current = beat
    registerTerminalSocketCallbacks({
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
    })

    // `pagehide` closes this socket (the server would otherwise keep a phantom
    // owner), `pageshow` and `resume` reopen it plain, and `freeze` parks it.
    // Unregistered below, so an unmounted pane is never revived by a page event.
    const unregisterLifecycle = registerPageLifecycle(pty)

    // A mount attach is a plain attach, through the same helper every other
    // non-take-over reopen uses.
    plainBounce(pty, takeoverIntent)
    beat.start()

    // Track the hidden-to-visible transition used by the attention grace. The
    // heartbeat owns retiming, and `pty.onOpen` owns the return resize.
    const noteVisibility = () => {
      const nowVisible = document.visibilityState === "visible"
      const returning = nowVisible && !prevVisibleRef.current
      visibleSinceRef.current = visibleSinceAfterTransition(
        prevVisibleRef.current,
        nowVisible,
        visibleSinceRef.current,
        Date.now(),
      )
      prevVisibleRef.current = nowVisible
      // The belt under the socket's rule that a pty socket never opens hidden.
      // An open that did land hidden asserts no size (a resize frame is a claim)
      // and `pty.onOpen` has already gone, so the pane would watch a pty nobody
      // owns. Asking again costs one forced re-assert and is a no-op otherwise.
      if (returning && pty.isOpen && !resize.sizeSentSinceOpen()) {
        resize.resyncToForeground()
      }
    }
    document.addEventListener("visibilitychange", noteVisibility)

    // An animated layout gesture outside this pane moves the host box on every
    // frame, and the ResizeObserver would answer each with a fit. The gesture
    // holds for its whole transition, so those coalesce into one fit and one
    // resize frame at the geometry it settled on.
    const unregisterLayoutGesture = registerLayoutGestureHolder({
      hold: () => resize.setHolding(true),
      release: () => {
        resize.setHolding(false)
        resize.flushHeld()
      },
    })

    return () => {
      unregisterLayoutGesture()
      disposeTerminalLifecycle({
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
      })
    }
    // Re-runs only when the streamed target changes: every other input reaches
    // its closures through a stable container, channel or port, and listing a
    // component-body function here would rebuild the terminal every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, id, sessionId, ptyUrl])
}
