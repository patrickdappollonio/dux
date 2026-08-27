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
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import "@xterm/xterm/css/xterm.css"

import { inactiveCursorStyle } from "@/lib/composebar"
import { isApplePlatform } from "@/lib/platform"
import { handleTabGone, noteOwnPtyConnection } from "@/lib/store"
import { isTabGone } from "@/lib/agentTabs"
import {
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  tabPtyUrl,
  terminalSocketUrl,
} from "@/lib/ptySocket"
import {
  clampTerminalFontSize,
  loadTerminalFontsThenRefit,
  terminalFontFamily,
} from "@/lib/terminalFont"
import { shouldSendViewed, visibleSinceAfterTransition } from "@/lib/viewedPing"
import { createHeartbeat, type Heartbeat } from "@/lib/heartbeat"
import { onServerRunUnconfirmed } from "@/lib/serverRun"
import { registerPageLifecycle } from "@/lib/pageLifecycle"
import type { ConnState } from "@/lib/types"
import { suppressViewerReports } from "@/lib/suppressViewerReports"
import { registerAgentNotifications } from "@/lib/agentNotifications"
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
import {
  WHEEL_SCROLL_SENSITIVITY,
  xtermScrollbarWidth,
} from "./constants"
import { createResizeCoordinator } from "./resizeCoordinator"
import { createAttachReplay } from "./attachReplay"
import { plainBounce } from "./plainBounce"
import { createLinkPress } from "./linkPress"
import { registerTerminalInputWiring } from "./inputWiring"
import { registerTerminalTouchWiring } from "./touchWiring"

/// The streamed target: an agent tab, or a companion terminal of either owner.
/// `id` is the FOCUSED TAB id for an agent (the session-slot tab's equals
/// `sessionId`; an extra tab's does not) and the terminal id for a terminal.
export type TerminalTarget =
  | { kind: "agent"; id: string; sessionId: string }
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
  // else. For an agent, the session-slot tab (`id === sessionId`) uses the
  // session PTY route and an extra tab its own nested route; a terminal uses
  // its owner's nested route.
  const ptyUrl =
    target.kind === "agent"
      ? target.id === target.sessionId
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

    // Resolve the app's background token so the terminal canvas matches the
    // shadcn palette rather than using a hardcoded hex color.
    const rawBg = getComputedStyle(document.documentElement)
      .getPropertyValue("--background")
      .trim()
    // The CSS variable is an oklch / hsl value; xterm expects a hex string.
    // Resolve it by painting a 1×1 canvas with the variable.
    let resolvedBg = "#000000"
    try {
      const canvas = document.createElement("canvas")
      canvas.width = 1
      canvas.height = 1
      const ctx = canvas.getContext("2d")
      if (ctx && rawBg) {
        ctx.fillStyle = `oklch(${rawBg})`
        ctx.fillRect(0, 0, 1, 1)
        const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data
        resolvedBg = `#${r.toString(16).padStart(2, "0")}${g.toString(16).padStart(2, "0")}${b.toString(16).padStart(2, "0")}`
      }
    } catch {
      // Fallback silently: resolvedBg stays black.
    }

    // Apply the resolved bg to the padded host so the padding area matches the
    // canvas, making the padding feel like it belongs to the terminal rather
    // than being an external border.
    host.style.background = resolvedBg

    // xterm 6 draws a custom DOM scrollbar whose width is the `overviewRuler.width`
    // option (default 14). Drive it from the SAME `--xterm-scrollbar-width` CSS
    // var the button overlay reserves its gutter from, so the slimmed scrollbar
    // and the reserved space always agree (single source). Setting the option
    // also instantiates an overview-ruler canvas; index.css hides it (dux uses no
    // decorations, so it's always empty).
    const scrollbarWidth = xtermScrollbarWidth()

    // Stable for this mount, and read by the link machine as well as the key
    // handler further down: it decides which chord is the force-forward hatch
    // (Cmd on a Mac, Ctrl elsewhere) and which clipboard chords to intercept.
    const isMac = isApplePlatform()
    // THE LINK-PRESS MACHINE. Constructed before the terminal because its
    // `linkHandler` has to be passed INTO the constructor; it learns its
    // terminal immediately afterwards.
    const links = createLinkPress({
      hyperlinks: () => live.current.hyperlinks,
      isMac,
    })
    const fontFamily = terminalFontFamily(live.current.fontFamily)
    const fontSize = clampTerminalFontSize(live.current.fontSize)
    const term = new Terminal({
      fontFamily,
      fontSize,
      cursorBlink: true,
      // The unfocused caret. With the compose bar up xterm never holds focus,
      // so this is the ONLY caret that client ever renders; see
      // `inactiveCursorStyle` for why that flips it off the default outline.
      // Read from the ref because this mount effect is stable per target; the
      // dedicated effect below keeps an OPEN terminal in step with the live
      // value.
      cursorInactiveStyle: inactiveCursorStyle(live.current.composeActive),
      convertEol: false,
      scrollback: live.current.scrollbackLines,
      // One wheel notch = 3 lines of local scrollback (see the constant's doc).
      scrollSensitivity: WHEEL_SCROLL_SENSITIVITY,
      overviewRuler: { width: scrollbarWidth },
      theme: { background: resolvedBg },
      // When the app in the PTY enables mouse reporting, xterm forwards a drag to
      // the host instead of selecting locally. The escape hatch is a modifier that
      // forces a LOCAL selection: Shift on Linux/Windows (xterm default), but on
      // macOS xterm gates it behind Option AND this flag. Enable it so a Mac-browser
      // visitor can Option-drag to select and copy to THEIR clipboard rather than
      // the host's. See the `onMouseUp` mouse-capture hint below.
      macOptionClickForcesSelection: true,
      // Make OSC 8 hyperlinks the agent emits clickable. xterm 6's built-in
      // linkHandler resolves real OSC 8 links (unlike the web-links addon, which
      // regex-scans for bare URLs). Opened with noopener,noreferrer so the new
      // tab can't reach back into the app.
      //
      // WHICH activations count is decided by the pure `linkActivateAction`:
      // xterm's Linkifier fires this from a bare `mouseup` with no button or
      // click-count check, so without the filter the second click of a
      // double-click (the select-a-word gesture) opened a SECOND tab, and a
      // right-click opened one on top of dux's own paste. See the helper.
      //
      // `hover`/`leave` exist for the desktop intercept, not for decoration:
      // they are the only public read of "is there a link at this point", since
      // the OSC 8 uri lives in an internal service. All three come from the
      // link machine.
      linkHandler: links.linkHandler,
    })
    links.setTerminal(term)
    // This xterm is a VIEWER of a PTY that dux-core's alacritty_terminal already
    // drives and answers device/color queries for. Stop it from also answering
    // (and injecting duplicate replies back into the shared PTY via onData); see
    // suppressViewerReports. Install before open so it is armed before any byte.
    suppressViewerReports(term)
    // Bridge the agent's notification/clipboard OSC sequences to the browser,
    // mirroring the TUI host passthrough. Registered next to suppressViewerReports
    // so both viewer hooks are armed before the first byte.
    const disposeAgentNotifications = registerAgentNotifications(term, {
      enabled: () => live.current.webNotifications,
      title: () => live.current.notifyTitle,
      clipboardMode: () => live.current.clipboardPassthrough,
      // A stable per-target tag so repeat notifications from this agent/terminal
      // replace instead of stack.
      tag: () => `dux-agent-${id}`,
    })
    // OSC 8 hyperlink gate at the PARSER layer: when hyperlinks are disabled,
    // consume the sequence (return true) so xterm creates no clickable link and it
    // renders as plain text; when enabled, fall through (return false) to xterm's
    // own OSC 8 handler, whose links the `linkHandler` above then gates to http(s).
    // Live-toggleable via the ref; links created before a toggle persist until the
    // cells are rewritten.
    const disposeOsc8Gate = term.parser.registerOscHandler(
      8,
      () => !live.current.hyperlinks,
    )
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
    term.open(container)
    // A virtual keyboard must never autocorrect, autocomplete, autocapitalize, or
    // spellcheck into the PTY stream: a shell has no editable buffer for those to
    // fix, so they only inject garbage. xterm is documented to set some of these
    // on its hidden input, but the defaults are not reliable across xterm versions
    // and mobile browsers (autocorrect in particular still fires), so set all four
    // explicitly rather than trusting the library.
    if (term.textarea) {
      term.textarea.setAttribute("autocomplete", "off")
      term.textarea.setAttribute("autocorrect", "off")
      term.textarea.setAttribute("autocapitalize", "off")
      term.textarea.setAttribute("spellcheck", "false")
    }
    resize.fitAfterOpen()
    // THE LOCAL HALF of the divergence comparison. A pure observation: it never
    // fits and never sends, so the coordinator remains the one owner of both
    // (see its module doc). xterm fires this only when the grid really changed,
    // and the mount fit above has already happened, so seed it by hand first.
    noteLocalGrid({ rows: term.rows, cols: term.cols })
    const localGridSub = term.onResize(({ rows, cols }) =>
      noteLocalGrid({ rows, cols }),
    )
    termRef.current = term
    fitAddonRef.current = fit
    // Open synchronously against fallback metrics (above), then refit once the
    // bundled faces (and any user-named family) are ready. See
    // `loadTerminalFontsThenRefit` for why this happens AFTER open rather than
    // before it: awaiting fonts before opening would delay the PTY connection
    // on every mount for a benefit (correct first-frame metrics) that only
    // matters on a cold font cache.
    loadTerminalFontsThenRefit(
      term,
      termRef,
      () => resize.refitForFonts(),
      fontSize,
      fontFamily,
    )

    // Record this socket's connection id (the socket's first `connected` frame, and
    // again on every reconnect since the server allocates a fresh id per open) so
    // the `pty.owner` handler can compare a handover's claimer id against ours.
    pty.onConnected = (connectionId, owner, ownerEpoch, ownerDevice) => {
      connId.write(connectionId)
      // Register the id as one of OURS in the store, so the server-published
      // `input_owner` spine field can be compared against this client's own
      // identity by surfaces outside this pane (see `sessionActiveElsewhere`).
      noteOwnPtyConnection(connectionId, true)
      // SEED THE VERDICT from the server's own answer. This replaced a direct
      // claim send that used to live here (the deferred take-over): with the
      // take-over intent now riding the ordinary first-frame resize, there is
      // nothing left for this handler to send, and the coordinator's last
      // send exception died with it.
      //
      // What it does instead is the correction that makes the whole arc safe: a
      // plain claim is refused SILENTLY by the server now, so a foregrounded
      // arrival's optimistic "mine" would never be corrected and the pane would
      // render typing surfaces over a pty whose keystrokes are all dropped. The
      // handshake says who drives; the foreground guess survives only for
      // claiming an UNOWNED pty. The epoch rides along so the seed can defer
      // to a strictly newer `pty.owner` this client already applied off the
      // events socket (the two sockets have no ordering between them).
      // The owner's device label rides along too: it is this pane's only source
      // of a name when it merely attached, since no `pty.owner` will follow.
      seedOwnershipFromConnected(connectionId, owner, ownerEpoch, ownerDevice)
    }

    // THE REMOTE HALF: the grid the child is actually drawing for, reported by
    // the `connected` handshake at attach and by a `size` event on every applied
    // resize thereafter. The flag says which, because they mean different
    // things: an attach is already sized against the handshake's answer, while a
    // change after it is what a diverged viewer heals from.
    pty.onPtyGrid = (grid, fromHandshake) => {
      // ADOPT BEFORE ARMING. In viewer mode this re-grids the terminal to the
      // PTY's geometry, and it must happen before `noteRemotePtyGrid` schedules
      // the heal bounce, so the replay that bounce brings back is parsed at the
      // child's own geometry rather than at the one this window happened to
      // have. (The reconnect's handshake re-reports the same grid, which is why
      // the adopt is idempotent.)
      resize.noteRemoteGrid(grid)
      noteRemotePtyGrid(grid, fromHandshake)
    }

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
    // A run this tab can no longer vouch for retires the replay dedupe's
    // high-water mark: the server's generation counter restarts with the
    // process, so a surviving mark would drop the new run's replay whole and
    // uncover the previous run's screen. See `lib/serverRun.ts`.
    const unsubscribeRunProbe = onServerRunUnconfirmed(() => {
      attach.forgetAppliedGeneration()
    })
    // THE COVER CLEARS HERE, and nowhere else. Not at WebSocket open, which is
    // the hole this closes: the pane used to drop its reconnect cover the moment
    // the socket opened, while the screen only exists once the server's replay
    // frame has been PARSED, so a socket that opened and stayed healthy without
    // ever sending a replay left the pane drawing nothing at all.
    attach.onReplayApplied((epoch) => noteReplayApplied(epoch))
    // On every (re)open the server replays a fresh repaint as the first binary
    // frame; re-arm the first-frame resize so a reconnect re-fits and re-asserts
    // this viewport's size (the same handling the very first open gets). A
    // reconnect (any open after the first) also arms the buffer reset above so the
    // replayed scrollback replaces, rather than stacks on, the stale buffer.
    pty.onOpen = () => {
      // The server allocates a FRESH connection id per open, so the previous id is
      // stale the instant the socket reopens. Clear it now (not only on the next
      // `connected` frame): on reconnect a `pty.owner` over the separate
      // `/ws/events` socket can arrive before this socket's new `connected` frame,
      // and a stale id would make `isOwnerAfterHandover` misjudge ownership. With
      // it null, a pre-`connected` handover safely reads as non-owner and resolves
      // once the new `connected` frame lands (epoch dedup keeps the latest claim).
      // Retire the stale id from the store's own-connection set too: the server
      // has already released anything it owned, so keeping it would make a spine
      // field naming it read as "mine" when it no longer is.
      if (connId.read() !== null) {
        noteOwnPtyConnection(connId.read() as string, false)
      }
      connId.write(null)
      setReconnecting(false)
      // A heal bounce (if this open is one) has landed; let the next grid
      // change arm another.
      noteSocketOpen()
      // The next binary frame is this open's scrollback replay: arm the repaint
      // handling (generation-drop + resize). Only opens AFTER the first also reset
      // the buffer first, since the first open starts from an empty terminal.
      // Arm the repaint handling for this open (generation dedupe, and the
      // reset a reconnect needs), and tell the coordinator which first-frame
      // plan it takes: the very first open jiggles, a reconnect must NOT (an
      // unchanged size would double-repaint the agent on every mobile
      // reconnect) and sends a single plain resize instead.
      const { firstOpen, epoch } = attach.noteOpen()
      resize.noteOpen(firstOpen)
      // A fresh attach epoch: the pane re-covers and its patience starts again
      // from zero, because the previous open's wait says nothing about this one.
      noteAttachEpoch(epoch)
      resetReplayWait()
      // A reopened socket moots whatever beat was outstanding on the old one.
      beat.reset()
      // THE SIZE HALF OF A RETURN, moved here from the visibility listener. It
      // used to run on every visibility signal, which meant it could fire at a
      // DEAD socket and silently book a size as delivered that never went out.
      // An open is the moment the question is both answerable and worth asking:
      // another client may have resized the shared pty while this tab was away.
      //
      // THE LIVE-SOCKET CASE IS DELIBERATELY NOT KEPT ALONGSIDE. It was there
      // for two reasons and attach-never-steals retired the second one outright:
      // another client can no longer resize a pty this pane owns, because a
      // plain resize against an owned pty is refused, so if somebody else's
      // geometry is on this pty then they TOOK it and this pane is a watcher,
      // which must assert no size at all. The first reason, this tab's own
      // layout having moved while hidden with rAF throttled, heals on its own:
      // the deferred fit runs when rAF resumes. Stated rather than measured, and
      // the symptom if it is wrong would be a pane that stays at its
      // pre-background geometry until the next real resize or reconnect.
      resize.resyncToForeground()
    }
    // The socket dropped and is retrying: surface the non-blocking reconnect state.
    pty.onReconnecting = () => {
      setReconnecting(true)
      // The dropped socket's connection id is dead server-side (its ownerships
      // were released on disconnect); retire it from the own-connection set so
      // a stale spine field naming it cannot read as "mine", and null the ref
      // like the onOpen and unmount paths do, so the two "is this id mine"
      // trackers never disagree (a take-over in this window then defers its
      // claim to the next `connected` frame instead of writing into a closed
      // socket).
      if (connId.read() !== null) {
        noteOwnPtyConnection(connId.read() as string, false)
        connId.write(null)
      }
    }
    // Connection-state transitions, routed to the ownership machine, which owns
    // both things that hang off them: the LOST state (`failed`, which now means a
    // terminal close code rather than a spent budget) and the take-over intent's
    // lifetime (ANY close retires it, so an automatic reconnect is always a plain
    // attach). The cue is the pane's own.
    pty.onConn = (connState) => {
      if (connState === "failed") setReconnecting(false)
      // A beat asked of a connection that is going away is not late, it is
      // moot. Retiring it here as well as on the reopen keeps the answer
      // deadline from running through the whole outage and dropping the retry
      // attempt that is trying to end it.
      if (connState !== "open") beat.reset()
      notePtyConn(connState)
    }
    // An extra tab's PTY route can go away out from under this client (another
    // client closed that tab); the closed WebSocket carries no HTTP status the
    // client can read, so a 404-forever route is otherwise indistinguishable
    // from a transient drop and `pty` would retry it forever with no escape (see
    // `isTabGone`). Only extra tabs need this: the session-slot tab's route
    // never goes away (closing it just detaches, it doesn't delete a row), and a
    // companion terminal isn't a tab at all.
    if (kind === "agent" && id !== sessionId) {
      pty.shouldRetry = () => !isTabGone(live.current.sessionTabs ?? [], id)
      pty.onGone = () => {
        handleTabGone(id)
      }
    }
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
    pty.onBeat = (n) => beat.noteAnswer(n)
    beatRef.current = beat

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
    // Track the hidden -> visible transition the attention grace is measured
    // from (see `viewedPing.ts`). This listener is now ONLY that: the grace
    // timer that used to fire an extra ping at the boundary is gone (the beat
    // runs every 2s while this device is owner-and-visible, and the heartbeat
    // RETIMES itself on the boundary through its own visibility listener rather
    // than waiting out a gap armed under the slow cadence, so the flag drops
    // within one fast cadence of the boundary without a second timer), and the
    // resize half of a return moved to `pty.onOpen`, where the socket is known
    // to be alive.
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

    return () => {
      resize.dispose()
      // An armed take-over dies with the mount it was armed in. This covers
      // unmount AND a switch to a different target: the intent names no pty of
      // its own, so carrying it across would flag the first resize of a
      // completely different terminal as a take-over of that one. Losing an
      // in-flight take-over to a target switch is the accepted cost, and the
      // button is one tap away on the new target.
      takeoverIntent.clear()
      // The pane must not re-grid a disposed terminal through a port still
      // pointing at this mount's coordinator, so retire it BEFORE the terminal
      // goes.
      if (viewerRegridRef.current !== null) viewerRegridRef.current = null
      beat.stop()
      if (beatRef.current === beat) beatRef.current = null
      unregisterLifecycle()
      unsubscribeRunProbe()
      links.dispose()
      inputWiring.dispose()
      touchWiring.dispose()
      document.removeEventListener("visibilitychange", noteVisibility)
      localGridSub.dispose()
      // DISPOSE this target's PTY socket, never merely close it: the pane is
      // gone, so its wake signals and its run-identity gate subscription go with
      // it. (A `close()` here would leave a socket that a window event could
      // revive into a pane that no longer exists.) Clear the active-socket
      // registration ONLY if it still points at this one. A
      // focus switch swaps panes; whichever order React runs old-cleanup vs
      // new-effect, the guard ensures we never null out the incoming pane's
      // registration (it has already replaced ours by the time we'd clear it).
      // The socket dies with the pane, and so does its connection id: retire
      // it from the store's own-connection set (the server releases anything
      // it owned the moment the socket closes).
      if (connId.read() !== null) {
        noteOwnPtyConnection(connId.read() as string, false)
        connId.write(null)
      }
      pty.dispose()
      if (ptyRef.current === pty) ptyRef.current = null
      if (getActivePtySocket() === pty) setActivePtySocket(null)
      termRef.current = null
      fitAddonRef.current = null
      disposeAgentNotifications()
      disposeOsc8Gate.dispose()
      term.dispose()
    }
    // The lifecycle owns the terminal's whole lifetime and re-runs ONLY when the
    // streamed target changes. Every other input reaches its closures through
    // the live-settings container, a channel, or a port, all of which are
    // stable; listing a component-body function here would tear the terminal
    // down and rebuild it on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, id, sessionId, ptyUrl])
}
