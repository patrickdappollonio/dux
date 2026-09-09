// Owner mode coordinates debounced xterm fits with PTY resizes; viewer mode only
// adopts the driver's grid. Gesture holds suspend the fit/send pair together.
import type { Terminal } from "@xterm/xterm"
import type { FitAddon } from "@xterm/addon-fit"

import { firstFrameResizePlan } from "@/lib/firstFrameResize"

import { RESIZE_SEND_DEBOUNCE_MS } from "./constants"

/// How long after the first frame a session that emits none is sized anyway.
const INITIAL_RESIZE_FALLBACK_MS = 250

/// The gap between the two halves of the first-open jiggle.
const JIGGLE_STEP_MS = 60

/// The settle window before a foreground return re-asserts the size.
const FOREGROUND_RESYNC_MS = 150

export type ResizeCoordinatorDeps = {
  term: Terminal
  fit: FitAddon
  /// The socket's own answer to "did that frame actually go out". Two things
  /// swallow a resize silently and neither raises anything, so the record of
  /// what the PTY has been told is built from this and nothing else.
  sendResize: (rows: number, cols: number) => boolean
  /// The ownership verdict, read live: a resize frame IS a claim server-side,
  /// so a read-only observer (and a backgrounded tab) drives nothing.
  isOwner: () => boolean
  /// Whether the frame `sendResize` just wrote carried the take-over flag. Read
  /// immediately after a confirmed send, and only then. A flagged frame is a
  /// REQUEST the server may refuse whole, so its geometry is not booked until
  /// the pty reports it back; see `sendOwned`.
  lastSendWasFlagged?: () => boolean
  /// The observed layout (the pane's HOST; see `start`) moved while in VIEWER
  /// mode. There is no fit to run, but the font shrink is computed from that
  /// box, so the pane recomputes it here. Called from the same ResizeObserver
  /// callback the fit would have used, so the two modes react to a layout
  /// change in one place.
  onViewerLayout: () => void
}

export type ResizeCoordinator = {
  /// The grid that follows `term.open()`, before the fonts have landed and
  /// before anything else is wired. It is here rather than at the call site so
  /// that "every fit goes through the coordinator" is literally true; there is
  /// no gesture and no socket yet, so it can never be held.
  fitAfterOpen: () => void
  /// Start observing the given element and subscribe to xterm's own resize
  /// event, then take the mount-time fit and seed the dedupe from it. The pane
  /// passes the host, never the container xterm opened into: the below-floor
  /// overflow branch pins the container to the adopted grid's pixel size, and
  /// an observer on a pinned box goes deaf exactly when the watcher needs a way
  /// out of pan mode.
  start: (observed: Element) => void
  /// A (re)open landed. `firstOpen` decides the first-frame plan: the very
  /// first open jiggles, every reconnect sends a single plain resize. This is
  /// also where the open's clock starts: the previous open's adopted-grid hold
  /// and no-first-frame fallback are dropped and a fresh fallback is armed.
  noteOpen: (firstOpen: boolean) => void
  /// Whether the next written chunk should carry the first-frame callback.
  /// The attach machine asks so it can pass the callback only when it means
  /// something, exactly as the hand-written version did.
  needsFirstFrameResize: () => boolean
  /// The first PTY frame after a (re)open has fully parsed: fit and notify.
  firstFrameLanded: () => void
  /// Force-resend the current size, bypassing the dedupe, once xterm's write
  /// queue has drained. The PTY is shared, so another client may have resized
  /// it while this tab was away and the cached size would wrongly suppress the
  /// re-assert.
  resyncToForeground: () => void
  /// Whether any resize frame has actually gone out since the current open.
  /// The pane asks on its way back to the foreground: an open that landed while
  /// the page was hidden asserted nothing, because a resize frame is a claim
  /// and a hidden tab is not the owner, and nothing else re-asks the question.
  sizeSentSinceOpen: () => boolean
  /// The gesture hold, the seam with the touch machine. `setHolding` mirrors
  /// "a touch scroll is in flight"; `flushHeld` releases whatever was held,
  /// refit first, exactly one fit.
  setHolding: (holding: boolean) => void
  flushHeld: () => void
  /// The owner-gated, dedupe-recording send, for the first-frame plan's own use
  /// and for the foreground re-assert.
  sendOwned: (rows: number, cols: number) => boolean
  /// The late refit once the bundled faces land, whose right answer differs by
  /// mode: fit the container, or recompute the watcher's font shrink.
  refitForFonts: () => void
  /// Record the PTY's own grid as the wire last reported it (the `connected`
  /// handshake, then every `size` event) and adopt it: always from the
  /// HANDSHAKE, because the replay that follows it was drawn for that grid,
  /// and afterwards only in VIEWER mode. Null means the server could not say,
  /// which is never read as agreement: the last grid it DID report stands.
  noteRemoteGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake?: boolean,
  ) => void
  /// Re-assert the recorded remote grid. Idempotent (a same-size
  /// `term.resize` is skipped) and a no-op outside viewer mode, so the pane
  /// may call it after anything that could have disturbed the grid: a
  /// demotion, a font change, a relayout.
  applyViewerGrid: () => void
  dispose: () => void
}

export function createResizeCoordinator(
  deps: ResizeCoordinatorDeps,
): ResizeCoordinator {
  const { term, fit, sendResize, isOwner, onViewerLayout } = deps
  const lastSendWasFlagged = deps.lastSendWasFlagged ?? (() => false)

  // Viewer mode, derived and never latched: anybody who is not the driver.
  // Named because "not the owner" is the reason at every decision point that
  // asks it, not an accident of this expression.
  const viewerMode = () => !isOwner()

  let lastRows = 0
  let lastCols = 0
  let fitFrame = 0
  let sendTimer: ReturnType<typeof setTimeout> | undefined
  let resyncTimer: ReturnType<typeof setTimeout> | undefined
  let jiggleTimer: ReturnType<typeof setTimeout> | undefined
  let initialResizeFallback: ReturnType<typeof setTimeout> | undefined
  let resizeSub: { dispose: () => void } | null = null
  let ro: ResizeObserver | null = null

  // Set when the debounced PTY resize came due while a touch-scroll gesture was
  // still in flight. A resize is a SIGWINCH, and landing one inside the
  // forwarded wheel-report stream corrupts a mouse-tracking alt-screen pager's
  // repaint with duplicated rows that persist, since an alt-screen has no
  // client scrollback to re-sync from.
  let resizeHeldByGesture = false
  // The local refit's half of the same hold: a measured xterm resize resets the
  // scrolling region on both buffers, so holding only the SIGWINCH is not
  // enough.
  let fitHeldByGesture = false
  // The second hold source: the debounce window. Same atomic-pair rule as the
  // gesture hold.
  let fitHeldByDebounce = false
  // Whether a debounced send is armed and therefore holding the fit. Cleared at
  // the top of the settle, never by `clearTimeout` alone.
  let debouncePending = false
  // The one deferred DIRECT resize request, if any.
  let heldResizeSend: (() => void) | null = null
  // Mirrors "a touch scroll is in flight", written by the touch machine.
  let holding = false
  // Whether any resize frame has crossed the wire since the current open; see
  // the port of the same name.
  let sizeSentSinceOpen = false
  // The PTY's own grid as the wire last reported it, or null while nothing
  // has. Recorded in BOTH modes (an owner is told its own applied grid too),
  // so a demotion has something to adopt immediately rather than waiting for
  // the next `size` event.
  let remoteGrid: { rows: number; cols: number } | null = null
  // Set while this open has taken the pty's grid off the handshake and the
  // replay drawn for it has not been parsed yet. Adopting alone is not enough:
  // the mount's observer callback, the bundled-font refit and container
  // settling all fit the terminal back to this viewport, which puts the grid
  // back exactly where the replay loses its lines. So every local fit is
  // refused for the width of the window, closed by `firstFrameLanded` or, for a
  // session that emits no first frame, by the fallback timer behind it.
  let holdingAdoptedGrid = false
  // Which open the hold and the fallback timer behind it belong to: a timer
  // armed under the previous open, firing during this one, would release a hold
  // it never took and drop this replay into a grid it was not drawn for.
  let openEpoch = 0
  // Whether THIS open's replay has actually been parsed. Deliberately not
  // `initialResizeDone`, which the fallback latches too: the fallback coming
  // due says the connection went quiet, never that the replay is on screen,
  // and the hold is owed to the replay.
  let replayParsed = false

  // Every local refit goes through here, because viewer mode has none: a
  // watcher's grid is the PTY's, adopted from the wire, and fitting it to this
  // container is the divergence the faithful view exists to remove.
  //
  // `evenAsViewer` is the one exception: a watcher the wire has named no grid
  // for has nothing to be faithful to, so its own viewport is the only geometry
  // it has. It still sends nothing.
  const runFit = (opts?: { evenAsViewer?: boolean }) => {
    if (!opts?.evenAsViewer && viewerMode()) return
    if (holdingAdoptedGrid) return
    fit.fit()
  }

  // Whether the wire has named a usable grid for this pty at all.
  const hasRemoteGrid = () =>
    !!remoteGrid && remoteGrid.rows > 0 && remoteGrid.cols > 0

  // Take the recorded grid, whoever is driving. Idempotent, and it books what
  // it adopted: the re-grid comes back through xterm's own resize event and
  // arms the debounced send, and a resize frame is a claim, so telling the
  // child its own size back says nothing and costs it a SIGWINCH repaint.
  const adoptRemoteGrid = () => {
    const grid = remoteGrid
    if (!grid) return
    if (grid.rows <= 0 || grid.cols <= 0) return
    lastRows = grid.rows
    lastCols = grid.cols
    if (term.rows === grid.rows && term.cols === grid.cols) return
    term.resize(grid.cols, grid.rows)
  }

  // The same adoption, gated on the mode. A no-op outside viewer mode, so
  // callers never have to ask which mode they are in.
  const applyViewerGrid = () => {
    if (!viewerMode()) return
    adoptRemoteGrid()
  }

  // Records what the PTY has been told, and only what actually went out. The
  // owner gate here and the socket (which discards a frame whenever the
  // WebSocket is not OPEN) both swallow a resize silently, and a swallowed send
  // booked as sent is worse than no send: the dedupe then suppresses the
  // re-assert forever and the child keeps drawing for a viewport nobody sees.
  // What the server does with a frame it received is its own business, so this
  // records "written to the socket" and claims nothing more.
  //
  // A steady-state resize by the current owner changes no owner and arms no
  // handover; only an ownership-acquiring claim does, and a claim always runs
  // with the verdict already flipped to "mine", so it passes this gate.
  //
  // A flagged frame is the one frame booked by the answer rather than the send.
  // A self-succession is refused routinely (that is what its compare-and-swap
  // is for) and a refusal applies no geometry at all, so booking it would let
  // the dedupe suppress every re-assert of that size. Its confirmation is the
  // pty's own reported grid; see `noteRemoteGrid`. A plain refusable frame is
  // still booked on the send, with the foreground resync as the recovery.
  const sendOwned = (rows: number, cols: number): boolean => {
    if (!isOwner()) return false
    if (!sendResize(rows, cols)) return false
    // A frame really went out on this open. Recorded even for a flagged one,
    // whose GEOMETRY is deliberately not booked below: the question this
    // answers is "has this open asked the size question at all", which a
    // take-over request asks as loudly as a plain resize does.
    sizeSentSinceOpen = true
    if (lastSendWasFlagged()) return true
    lastRows = rows
    lastCols = cols
    return true
  }

  // The debounce settling. Releases the pair it held: the refit runs first, at
  // the final container size, and the child's notification follows it.
  const sendSize = () => {
    debouncePending = false
    // Never land a SIGWINCH inside an active touch-scroll's wheel-report
    // stream: hold the send and let the lift flush it after the finger goes.
    // A gesture outliving the debounce inherits the parked fit too, so the pair
    // stays together rather than the fit escaping through this settle.
    if (holding) {
      resizeHeldByGesture = true
      if (fitHeldByDebounce) {
        fitHeldByDebounce = false
        fitHeldByGesture = true
      }
      return
    }
    if (fitHeldByDebounce) {
      fitHeldByDebounce = false
      // Coalesced, last geometry wins: `fit.fit()` reads the container now, so
      // however many observer callbacks were parked, this is one fit at the
      // size the drag ended on. It re-enters `armDebounce` through xterm's own
      // resize event, which is a no-op send one window later.
      runFit()
    }
    if (term.rows !== lastRows || term.cols !== lastCols) {
      sendOwned(term.rows, term.cols)
    }
  }

  const armDebounce = () => {
    clearTimeout(sendTimer)
    debouncePending = true
    sendTimer = setTimeout(sendSize, RESIZE_SEND_DEBOUNCE_MS)
  }

  // The ResizeObserver's local refit: do it now, or mark it held. Never fit
  // while either hold is on.
  const fitOrHold = () => {
    if (holding) {
      fitHeldByGesture = true
      return
    }
    if (debouncePending) {
      fitHeldByDebounce = true
      return
    }
    runFit()
  }

  // A direct resize request (the first-frame jiggle, the reconnect resize, the
  // foreground resync): refit and notify the child now, or defer BOTH halves to
  // gesture end. These paths bypass the debounce on purpose, so each has to
  // route through the hold explicitly or the pair comes apart again.
  //
  // Direct resize paths remain internal so every external resize follows the
  // coordinator's ownership and hold rules.
  const fitAndSend = (send: () => void) => {
    if (holding) {
      fitHeldByGesture = true
      // First one wins while held: plain resize sends are interchangeable, each
      // re-reading the live geometry, but a later one overwriting a parked
      // first-open jiggle would skip the redraw nudge for that open.
      heldResizeSend = heldResizeSend ?? send
      return
    }
    // A direct send fits for itself, which satisfies anything the debounce
    // window had parked; leaving the flag set would fit a second time at the
    // settle for no reason.
    fitHeldByDebounce = false
    runFit()
    send()
  }

  // Defer the initial PTY resize until the first PTY frame after each (re)open
  // has rendered. That frame is the server's repaint, a static snapshot at the
  // PTY's current size, so resizing before it lands races a half-painted buffer
  // and leaves the cursor and the bottom-anchored prompt in the wrong rows.
  // xterm's write callback fires once the frame is parsed.
  let initialResizeDone = false
  // Whether the NEXT first-frame resize should jiggle (very first open) or send
  // a single plain resize (a reconnect). `noteOpen` sets it before the first
  // frame lands; it defaults to `true` so the very first open still jiggles
  // even in the pathological case where the fallback timer beats the open.
  let firstFrameIsFirstOpen = true

  // The resize plan itself, run by the replay's write callback and, for a
  // connection that says nothing after its handshake, by the fallback timer.
  const runFirstFrameResize = () => {
    // Released before the guard below, not after: a repeat call is a no-op for
    // the resize plan, but the hold must go whichever call gets here first.
    holdingAdoptedGrid = false
    if (initialResizeDone) return
    initialResizeDone = true
    // Fit and notify as one pair, deferred whole if a touch gesture is in
    // flight.
    fitAndSend(() => {
      // Attaching while foregrounded claims ownership by sending our size. The
      // server broadcasts a `pty.owner` carrying our connection id; the
      // handover handler recognises it as ours by id, so no echo bookkeeping is
      // needed here. A backgrounded observer is not the owner, so the sends
      // below no-op.
      if (firstFrameResizePlan(firstFrameIsFirstOpen) === "jiggle") {
        // First open only. A same-size resize is a kernel no-op, so when the
        // PTY already matches this viewport the agent never repaints and the
        // initial snapshot stays on screen with the cursor and input box
        // misplaced. Nudging the width down one column and back makes two real
        // winsize changes, so the kernel raises SIGWINCH and the agent redraws
        // at the correct size.
        //
        // One jiggle per open: a slow handshake can re-owe the first-frame
        // resize after the fallback has run the plan once (see
        // `noteRemoteGrid`), and that second pass is a plain resize.
        firstFrameIsFirstOpen = false
        sendOwned(term.rows, Math.max(1, term.cols - 1))
        jiggleTimer = setTimeout(() => {
          // The continuation is its own direct send, so it takes the same hold:
          // a gesture that started inside the window would otherwise catch this
          // SIGWINCH mid-stream.
          fitAndSend(() => sendOwned(term.rows, term.cols))
        }, JIGGLE_STEP_MS)
      } else {
        // Reconnect: the PTY is still alive at its prior size. Jiggling would
        // force two full-screen repaints on every reconnect, and mobile
        // reconnects constantly. A single resize still re-asserts ownership and
        // raises one SIGWINCH only if the viewport really changed.
        sendOwned(term.rows, term.cols)
      }
    })
  }

  /// The replay's own write callback: this open's replay is on screen, so the
  /// hold it was owed is discharged and the terminal sizes itself again.
  const firstFrameLanded = () => {
    replayParsed = true
    clearTimeout(initialResizeFallback)
    runFirstFrameResize()
  }

  // The no-first-frame fallback, measured from whenever the connection last
  // spoke and keyed to the open that armed it. A superseded open answers for
  // nobody: its timer releasing this open's hold is the same lost lines by
  // another route.
  const armInitialFallback = () => {
    clearTimeout(initialResizeFallback)
    const forEpoch = openEpoch
    initialResizeFallback = setTimeout(() => {
      if (forEpoch !== openEpoch) return
      runFirstFrameResize()
    }, INITIAL_RESIZE_FALLBACK_MS)
  }

  return {
    fitAfterOpen() {
      runFit()
    },
    start(observed) {
      // Geometry is reported to the PTY from exactly one place: xterm's own
      // resize event. A local re-grid has more causes than the ResizeObserver
      // (the bundled fonts landing re-grids the terminal with no container
      // resize anywhere), and each must reach the child or it draws for a
      // geometry the browser is not rendering, leaving duplicated output in the
      // scrollback that no later resize heals. xterm fires this only on a real
      // grid change, and the debounce plus the dedupe keep a no-op off the wire.
      resizeSub = term.onResize(() => armDebounce())
      // Local fit so the canvas matches this viewport right away, and seed the
      // dedupe so the ResizeObserver's initial observe callback does NOT send a
      // (racing) resize before the first paint. The initial PTY resize is
      // deferred to the first-frame handler.
      runFit()
      lastRows = term.rows
      lastCols = term.cols
      // No fallback is armed here. It is armed when the connection speaks
      // (`noteOpen`, restarted by the handshake), because it asks whether this
      // connection has gone quiet and the pane mounts before there is one:
      // armed at mount it can beat the handshake on a slow link, latch the
      // first-frame resize, and leave the replay landing in a grid it was not
      // drawn for.
      ro = new ResizeObserver(() => {
        cancelAnimationFrame(fitFrame)
        // Viewer mode: the observed box decides nothing about the grid, only
        // how small the font must be for the PTY's grid to fit. In the same
        // callback the fit would have used, so two observers can never
        // disagree about when a resize happened.
        if (viewerMode()) {
          fitFrame = requestAnimationFrame(() => onViewerLayout())
          return
        }
        // Through the hold, never a bare fit: a refit landing mid-touch-gesture
        // resets the child's scrolling region under it.
        fitFrame = requestAnimationFrame(() => fitOrHold())
        armDebounce()
      })
      ro.observe(observed)
    },
    noteOpen(firstOpen) {
      openEpoch++
      initialResizeDone = false
      sizeSentSinceOpen = false
      // A new open owes a new handshake, so the previous one's hold is not this
      // one's; the new handshake takes it again, and the clock this open runs
      // on starts here.
      holdingAdoptedGrid = false
      replayParsed = false
      armInitialFallback()
      // A reconnect must NOT jiggle: an unchanged size would double-repaint the
      // agent on every mobile reconnect.
      firstFrameIsFirstOpen = firstOpen
    },
    needsFirstFrameResize: () => !initialResizeDone,
    firstFrameLanded,
    resyncToForeground() {
      // Debounced and gated on xterm draining its write queue, because a
      // foreground return can coincide with the replay still streaming and
      // resizing mid-replay corrupts the scroll position. The send is forced
      // rather than routed through the deduped `sendSize`: another client may
      // have set the PTY's current size, so the cached record would wrongly
      // suppress the re-assert, and a same-size resize is a kernel no-op.
      clearTimeout(resyncTimer)
      resyncTimer = setTimeout(() => {
        term.write("", () => {
          // The pair again: a foreground return that lands mid-gesture defers
          // both halves to the lift rather than refitting under the finger.
          fitAndSend(() => sendOwned(term.rows, term.cols))
        })
      }, FOREGROUND_RESYNC_MS)
    },
    sizeSentSinceOpen: () => sizeSentSinceOpen,
    setHolding(next) {
      holding = next
    },
    flushHeld() {
      // Release the resize pair the gesture held back: one refit, at the final
      // container size, then the child's notification. A direct-send path does
      // not fit for itself while held, so this flush cannot double-fit.
      const pendingSend = heldResizeSend
      heldResizeSend = null
      // Either hold is discharged by the one fit; leaving the debounce's flag
      // set would fit a second time at the settle.
      if (fitHeldByGesture || fitHeldByDebounce) {
        fitHeldByGesture = false
        fitHeldByDebounce = false
        runFit()
      }
      pendingSend?.()
      // A debounced send the gesture held back: the wheel-report stream ends
      // with the finger, so re-arming the normal debounce here sends one
      // resize, at the final size, after the stream.
      if (resizeHeldByGesture) {
        resizeHeldByGesture = false
        armDebounce()
      }
    },
    sendOwned,
    refitForFonts() {
      if (viewerMode()) {
        onViewerLayout()
        // A watcher the wire has named no grid for is not rendering
        // faithfully: the pane's shrink has no grid to compute against, so
        // this viewport is the only geometry it has and new cell metrics
        // really do need a fit. It sends nothing, so it claims nothing.
        if (!hasRemoteGrid()) runFit({ evenAsViewer: true })
        return
      }
      // Through the guard, not around it: the faces land while the attach is
      // still in flight, so this fit would otherwise put the adopted grid back
      // under the replay. The hold's release fits for it.
      runFit()
    },
    noteRemoteGrid(grid, fromHandshake) {
      // Null is "the server could not say", never "it matches": the last grid
      // it DID report stands, which is the same rule `gridsDiverge` applies.
      if (grid) remoteGrid = grid
      // The confirmation half of the flagged-send rule above: a reported grid
      // is the pty's own geometry, so for the owner it answers whether the size
      // it asked for was applied, and it is what the dedupe should compare
      // against even when it is not what this client last asked for.
      if (grid && isOwner()) {
        lastRows = grid.rows
        lastCols = grid.cols
      }
      // The handshake is the one report a replay follows, so the owner adopts
      // it too. The replay is a repaint drawn for the pty's grid, absolute
      // cursor addressing and all: parsed in a taller terminal it parks the
      // cursor short of the bottom and the live bytes behind it overwrite the
      // last rows the replay drew, destroying them in the buffer for good.
      //
      // Adopting costs the owner nothing: its own first-frame resize follows
      // the replay's write callback and fits straight back to this viewport.
      // Every later report is this client's own resize echoed back, which only
      // a viewer adopts, or the echo would undo the fit that asked for it.
      //
      // A null handshake authorizes nothing: adopting some earlier
      // connection's grid would re-grid on a report that named no geometry and
      // hold it for a replay drawn at a size this handshake never claimed.
      if (fromHandshake && grid) {
        adoptRemoteGrid()
        // Only while a replay is still owed. A handshake whose replay is
        // already on screen has nothing left to protect, and a hold nobody
        // releases would stop this pane fitting for the rest of its life.
        //
        // The question is the replay, never `initialResizeDone`: on a slow link
        // the fallback can come due before the handshake arrives, and its latch
        // does not mean the replay is on screen. So this open re-owes its
        // first-frame resize.
        if (!replayParsed) {
          holdingAdoptedGrid = true
          initialResizeDone = false
          // AND THE OPEN'S CLOCK RESTARTS HERE, because the replay behind this
          // handshake is still in flight and the fallback's question is "has
          // this connection gone quiet".
          armInitialFallback()
        }
      } else {
        applyViewerGrid()
      }
    },
    applyViewerGrid,
    dispose() {
      cancelAnimationFrame(fitFrame)
      resizeSub?.dispose()
      resizeSub = null
      clearTimeout(sendTimer)
      clearTimeout(resyncTimer)
      clearTimeout(jiggleTimer)
      clearTimeout(initialResizeFallback)
      ro?.disconnect()
      ro = null
    },
  }
}
