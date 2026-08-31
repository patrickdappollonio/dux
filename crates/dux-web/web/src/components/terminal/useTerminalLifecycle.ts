// THE ONE LIFECYCLE OWNER for the terminal pane.
//
// It creates the terminal, opens the socket, wires every listener the pair
// needs, and tears all of it down. It re-runs ONLY when the streamed target
// changes, which is the property everything else here is arranged around: every
// closure it creates outlives the render that created it, so nothing may reach
// a closure by being captured. Read-only values come from the live-settings
// container, the three values the wiring writes come from named channels, and
// what remains (a focus call, the pane's own paste handler) is passed in as an
// explicit port.
//
// THE KEY IS THE TARGET, NOT THE ADDRESS. `ptyUrl` is derived here rather than
// handed in, because it is a function of the target and nothing else. The
// target's own parts are then real parameters of the wiring, not merely parts of
// a key: `kind` and `id` decide the tab-gone retry policy, the notification tag,
// and the pty field the upload route stamps.
//
// SURVIVING SUBSCRIPTIONS. Everything below registers inside the one effect and
// is disposed by the one cleanup. The pane keeps a small number of SEPARATE
// registration effects on purpose (the attach capability, the compose sink, the
// focus target, the ownership ledger); those are inventoried in `TerminalPane`
// beside their own code, so a reviewer can tell a deliberate subscription from a
// smuggled-back settings mirror.
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
  /// THE TWO VIEWER-VIEW PORTS, a mount-scoped idiom pointing in opposite
  /// directions.
  ///
  /// `viewerRegridRef` is INSTALLED here over this mount's coordinator: the
  /// pane calls it to re-assert the adopted grid after anything that could
  /// have disturbed it (a demotion, a font change), so a viewer re-grid stays
  /// a coordinator act rather than a lifecycle side effect.
  viewerRegridRef: RefObject<(() => void) | null>
  /// `viewerRelayoutRef` is installed by the PANE and read here: the
  /// coordinator's own ResizeObserver calls it instead of fitting while a
  /// watcher renders faithfully, because the host's size decides the shrink
  /// font and nothing else.
  viewerRelayoutRef: RefObject<(() => void) | null>
  live: LiveSettings
  mods: ModifierLatch
  ownership: OwnershipVerdict
  connId: ConnectionIdentity
  /// The PTY socket's connection state. Owns the LOST state and the take-over
  /// intent's lifetime; see `ownership.ts`'s `notePtyConn`.
  notePtyConn: (state: ConnState) => void
  /// Re-seed the ownership verdict from the `connected` handshake's `owner`
  /// field. The frame lands here; the decision lives in the ownership machine.
  /// `ownerDevice` is the handshake's `owner_device`, which seeds the take-over
  /// card's device name for a watcher that merely attached (a mere attach hears
  /// no `pty.owner` broadcast, so this frame is its only source of a name).
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
  /// The RENDERED ownership verdict. Read for exactly one thing: the periodic
  /// frame's cadence depends on it (2s while owner-and-visible, the configured
  /// heartbeat otherwise), and a cadence change has to clear the armed timer
  /// rather than wait it out.
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
  // The PTY socket URL for THIS target, derived from the target and nothing
  // else. For an agent, the session-slot tab uses the session PTY route and an
  // extra tab its own nested route; a terminal uses its owner's nested route.
  // Slot-ness is decided against the spine's `slotTabId`, because the slot tab's
  // id is generated and the session id is only a placeholder for it. Both forms
  // are valid for the slot tab now: the server serves it at its own
  // `/tabs/:tab/pty` address too, and the bare per-agent route is a convenience
  // alias onto the identical PTY. The choice below stays with the alias (no
  // behavior change, and it keeps the slot tab out of the per-tab socket quota);
  // the stable per-tab form is what tab promotion will move it to.
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
  // A take-over (or losing one) changes the beat's cadence, and the pending
  // timer was armed under the old one. Clearing and re-arming is what keeps the
  // engine's attention flag from staying lit a whole slow period past the
  // boundary; the hidden-to-visible half is the heartbeat's own listener.
  useEffect(() => {
    beatRef.current?.resync()
  }, [isOwnerRendered])

  useEffect(() => {
    const host = hostRef.current
    const container = containerRef.current
    if (!host || !container) return

    const terminalSetup = createTerminalSetup({ host, id, live })
    const { term, links, isMac } = terminalSetup
    // The dedicated PTY socket for THIS target: the agent's main provider PTY, or
    // a companion terminal's PTY (nested under its owning session). Opening it IS
    // the subscription: connecting an agent socket launches/resumes the provider,
    // exactly as the legacy `Subscribe` did. Registered as the active socket so the
    // macro picker can write to it; cleared on unmount. The byte feed and the
    // first-frame resize are wired further down (after the sizing state exists).
    // Opening a tab's socket launches it if it isn't running (resume is decided
    // server-side: it continues only when it's the sole tab coming up). A
    // dormant tab is never auto-mounted (App renders its card instead), so
    // reaching here for one is an intentional launch.
    const pty = new PtySocket(ptyUrl)
    ptyRef.current = pty
    setActivePtySocket(pty)
    // Constructed BEFORE the addon so the resize coordinator below can be given
    // the socket's own `sendResize`; `connect()` is still the explicit act
    // further down, and nothing reads the registration in between.
    const fit = new FitAddon()
    term.loadAddon(fit)
    // THE RESIZE COORDINATOR: the one owner of `fit.fit()` and of every frame
    // that tells the child its size. Nothing below refits or notifies except
    // through it (see its module doc for the one stated font exception).
    // Whether the frame the wrapper below just wrote carried the take-over flag.
    // Read by the coordinator immediately after the call, and by nothing else.
    let lastSendFlagged = false
    const resize = createResizeCoordinator({
      term,
      fit,
      // THE ONE PLACE A TAKE-OVER INTENT IS CONSUMED. Every resize frame the
      // pane sends passes through here, so whichever frame reaches the wire
      // first carries the flag; the intent does not care which one that is,
      // which is exactly why it is a flag and not a parked closure (a closure
      // was lost to the gesture coalescer and to a re-dropped socket, both
      // measured).
      //
      // Cleared only on a CONFIRMED write. `sendResize` answers whether the
      // frame really went out, and a socket that is CONNECTING or CLOSED
      // discards it silently; clearing the intent on a discarded frame would
      // spend the take-over on nothing and leave the user pressing a button
      // that does not work.
      sendResize: (rows, cols) => {
        const takeover = takeoverIntent.read()
        // The ordinary frame is left untouched, arity included: a take-over is
        // rare and a resize is not, so the common call stays exactly the call
        // it always was rather than growing a `false` every frame.
        // A SELF-SUCCESSION names the ghost it expects to displace; a PRESSED
        // take-over names nobody, because a press may take from anyone. The
        // server refuses the transfer when the named ghost no longer holds the
        // pty, and this client then lands as a watcher with the card, exactly
        // like a refused plain resize.
        const sent = takeover
          ? pty.sendResize(rows, cols, true, takeoverIntent.expectedOwner())
          : pty.sendResize(rows, cols)
        if (sent && takeover) takeoverIntent.clear()
        // The coordinator books a plain send's geometry immediately and a
        // FLAGGED one's only once the pty reports it back, because a claim can
        // be refused whole. It reads this right after the call above.
        lastSendFlagged = sent && takeover
        return sent
      },
      lastSendWasFlagged: () => lastSendFlagged,
      isOwner: () => ownership.read(),
      onViewerLayout: () => viewerRelayoutRef.current?.(),
    })
    // The pane's handle on this mount's grid adoption (see the port's doc).
    viewerRegridRef.current = () => resize.applyViewerGrid()
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

    // THE LINK-PRESS MACHINE owns the capture-phase intercept, the hover
    // cache, the one opener and the activation counter the touch probe reads.
    // See its module doc for why the decision is made at PRESS time, why the
    // phase and the `stopPropagation` are load-bearing, and why it abstains
    // entirely under the force-local-selection modifier.
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

    // THE SIZING PLUMBING, all of it, is the resize coordinator's: it
    // subscribes to xterm's own resize event (the ONE place geometry reaches
    // the PTY), takes the mount fit, seeds its dedupe from it, arms the
    // no-first-frame fallback, and starts observing the layout.
    //
    // It observes the HOST, not the container. The relayout's below-floor
    // overflow branch pins the CONTAINER to the adopted grid's pixel size, and
    // a pinned box never moves with the window, so observing it left the
    // overflow state deaf to every host and window resize: a watcher could
    // never leave pan mode until some unrelated event ran the relayout again.
    // The host's box is set by the pane layout and is never pinned. The owner
    // path is unchanged by this: the owner never pins the container, which is
    // `h-full w-full` of the host, so the two boxes resize together and the
    // observer fires for exactly the same layout changes it always did.
    resize.start(host)
    // THE ATTACH-AND-REPLAY MACHINE owns the (re)open's repaint: the
    // generation dedupe, the reset, the drain and its held chunks, and the
    // focus-report suppression window. See its module doc for each.
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
    // THE ONE PERIODIC CLIENT FRAME (see `lib/heartbeat.ts`): the viewed
    // semantics and an application-level beat, folded into one message on one
    // timer. There is never a second pinger. The beat exists because the
    // server's own WebSocket ping is send-only with no pong deadline, so it
    // cannot see the half-open socket a radio handoff leaves behind; a missed
    // answer drops the socket and lets the ORDINARY retry path reattach, PLAIN.
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

    // PAGE LIFECYCLE. `pagehide` closes this socket (a bfcache'd page holding an
    // open socket is evicted anyway, and the server would keep a phantom owner
    // until its next send failed), `pageshow` and Chromium's `resume` reopen it
    // plain, and `freeze` parks it. Unregistered by the cleanup below, so a pane
    // that unmounted is never revived by a page event.
    const unregisterLifecycle = registerPageLifecycle(pty)

    // Open the socket now that the byte feed and first-frame handling are
    // wired. A mount attach is a plain attach, so it goes through the same
    // helper every other non-take-over reopen uses; there is nothing armed this
    // early, and that is the point of it being the same call.
    plainBounce(pty, takeoverIntent)
    beat.start()

    // Re-assert THIS client's size whenever the tab or window returns to the
    // foreground. Two things can leave it stale on return:
    //  1. While hidden, rAF is throttled so the ResizeObserver's deferred fit()
    //     never ran, pinning the canvas and PTY to the pre-switch size.
    //  2. The PTY is SHARED across clients. Another client (typically a phone)
    //     may have resized it to its own dimensions while this tab was
    //     backgrounded or merely unfocused, so the PTY is now sized for that
    //     other viewport, not this one.
    // visibilitychange covers tab hide/show, but moving between a desktop and a
    // phone usually leaves the desktop tab "visible" the whole time, so we also
    // listen for window focus to catch the never-hidden case. The resize send is
    // FORCED (not routed through the deduped sendSize) because the PTY's current
    // size was set by the OTHER client, so our cached lastRows/lastCols would
    // wrongly suppress the re-assert. A same-size resize is a kernel no-op (no
    // SIGWINCH), so re-asserting an unchanged size never causes a spurious
    // redraw; a changed one makes the child redraw at this viewport's true size.
    //
    // The send is debounced (coalescing rapid focus/visibility flaps) and, like
    // the re-attach redraw above, gated on xterm draining its write queue: a
    // foreground return can coincide with the server's scrollback replay still
    // streaming in, and resizing mid-replay corrupts the scroll position. The
    // empty-write callback fires only once the queued writes have drained, so we
    // fit + resize against a settled buffer.
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
      // THE BELT under the socket's own rule that a pty socket never opens
      // hidden. If an open ever does land hidden, it asserts no size (a hidden
      // tab is not the owner, and a resize frame is a claim), and `pty.onOpen`
      // has already been and gone, so the pane would watch a pty nobody owns
      // for as long as it stayed on screen. Asking again on the first visible
      // moment of an open that has said nothing costs one forced re-assert and
      // is a no-op in every other state: a watcher's send is gated on
      // ownership, and an open that already sent its size never gets here.
      if (returning && pty.isOpen && !resize.sizeSentSinceOpen()) {
        resize.resyncToForeground()
      }
    }
    document.addEventListener("visibilitychange", noteVisibility)

    return () =>
      disposeTerminalLifecycle({
        resize,
        takeoverIntent,
        viewerRegridRef,
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
    // The lifecycle owns the terminal's whole lifetime and re-runs ONLY when the
    // streamed target changes. Every other input reaches its closures through
    // the live-settings container, a channel, or a port, all of which are
    // stable; listing a component-body function here would tear the terminal
    // down and rebuild it on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, id, sessionId, ptyUrl])
}
