// THE RESIZE COORDINATOR.
//
// Sizing has two halves with very different costs:
//  - LOCAL refits (`fit.fit()`) are cheap, so the canvas tracks the container
//    every frame while the user drags a divider or the window edge.
//  - PTY resizes are expensive: each one is a SIGWINCH that makes the child TUI
//    fully redraw. Sending them per-frame during a drag is the resize jitter.
//    So the send is DEBOUNCED (one resize with the final dimensions once the
//    drag settles) and deduplicated, since ResizeObserver also fires an initial
//    callback on observe.
//
// THE INVARIANT, and the reason this is a machine rather than a handful of
// closures: NO CALL SITE TOUCHES `fit.fit()` OR `sendResize` EXCEPT THROUGH
// THIS. It is precisely the rule whose absence caused the repeated-lines bug on
// phones. The local refit and the child's notification are ONE ATOMIC PAIR, and
// a touch gesture holds both or neither, because (measured, xterm 6.0.0)
// `Buffer.resize` sets `scrollBottom = newRows - 1` unconditionally and
// `scrollTop = 0`, over the normal AND the alt buffer, so every `fit.fit()`
// that changes the grid silently resets DECSTBM on both. Refitting mid-gesture
// hands a region-relative, mouse-tracking pager a viewer whose margins are gone
// while the child still paints for the old geometry, and its repaint stamps one
// line per forwarded wheel notch.
//
// It owns, in one place: the fit, the debounce, the dedupe, the first-frame
// plan (jiggle or single resize, including the jiggle's held continuation), the
// foreground re-assert, and the gesture hold.
//
// THERE IS NO LONGER AN EXCEPTION TO "no other call site sends". The take-over
// button used to be one: it called the socket's `sendResize` directly for the
// synchronous did-it-go-out boolean that told it whether to reopen a dead
// socket. Take-over is now a socket BOUNCE carrying an armed intent, so it
// sends nothing itself and the claim rides the reconnect's ordinary first-frame
// resize, through here like everything else. The intent is read and cleared in
// the `sendResize` the lifecycle hands this machine, which is the one confirmed
// write; this machine does not know the flag exists.
//
// ONE STATED EXCEPTION REMAINS, to "no other call site fits": the two FONT-driven
// refits, in `lib/terminalFont.ts` (the late refit once the bundled faces land)
// and in the pane's live font-preference effect. Both re-grid the terminal
// without asking this machine, and both are pre-existing and deliberate,
// because the metrics have moved and the canvas would otherwise be wrong. They
// are safe for the PTY half of the pair for the reason A4 exists: the grid
// change reaches the child through xterm's own `onResize`, which this machine
// subscribes to and debounces. They are NOT covered by the gesture hold, which
// is accepted: a font landing mid-touch-scroll is not a reachable sequence in
// the way a keyboard collapse is.
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
}

export type ResizeCoordinator = {
  /// The grid that follows `term.open()`, before the fonts have landed and
  /// before anything else is wired. It is here rather than at the call site so
  /// that "every fit goes through the coordinator" is literally true; there is
  /// no gesture and no socket yet, so it can never be held.
  fitAfterOpen: () => void
  /// Start observing the container and subscribe to xterm's own resize event,
  /// then take the mount-time fit and seed the dedupe from it.
  start: (container: Element) => void
  /// A (re)open landed. `firstOpen` decides the first-frame plan: the very
  /// first open jiggles, every reconnect sends a single plain resize.
  noteOpen: (firstOpen: boolean) => void
  /// Whether the next written chunk should carry the first-frame callback.
  /// The attach machine asks so it can pass the callback only when it means
  /// something, exactly as the hand-written version did.
  needsFirstFrameResize: () => boolean
  /// The first PTY frame after a (re)open has fully parsed: fit and notify.
  firstFrameLanded: () => void
  /// A DIRECT (non-debounced) resize request: refit and notify now, or defer
  /// BOTH halves to the gesture's lift. The freed-pty claim (a viewer taking an
  /// unowned pty when its driver disconnects) is the one external caller.
  directSend: (send: () => void) => void
  /// Force-resend the current size, bypassing the dedupe, once xterm's write
  /// queue has drained. The PTY is shared, so another client may have resized
  /// it while this tab was away and the cached size would wrongly suppress the
  /// re-assert.
  resyncToForeground: () => void
  /// The gesture hold, the seam with the touch machine. `setHolding` mirrors
  /// "a touch scroll is in flight"; `flushHeld` releases whatever was held,
  /// refit first, exactly one fit.
  setHolding: (holding: boolean) => void
  flushHeld: () => void
  /// The owner-gated, dedupe-recording send, for the first-frame plan's own use
  /// and for the foreground re-assert.
  sendOwned: (rows: number, cols: number) => boolean
  dispose: () => void
}

export function createResizeCoordinator(
  deps: ResizeCoordinatorDeps,
): ResizeCoordinator {
  const { term, fit, sendResize, isOwner } = deps

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
  // still in flight. A resize is a SIGWINCH, a full child repaint, and landing
  // one in the middle of the forwarded wheel-report stream corrupts a
  // mouse-tracking alt-screen pager's repaint (duplicated rows that PERSIST,
  // since an alt-screen has no client scrollback and nothing reconnects to
  // re-sync it). This is not exotic: the scroll-start blur collapses the soft
  // keyboard, `interactive-widget=resizes-content` then grows the viewport, and
  // the debounced resize fires under the finger.
  let resizeHeldByGesture = false
  // The LOCAL refit's half of the same hold. Holding only the SIGWINCH is not
  // enough; see the module doc for the measured reason.
  let fitHeldByGesture = false
  // The one deferred DIRECT resize request, if any.
  let heldResizeSend: (() => void) | null = null
  // Mirrors "a touch scroll is in flight", written by the touch machine.
  let holding = false

  // It records what the PTY has been told, and it records only what actually
  // went out. TWO things can swallow a resize and neither raises anything: the
  // owner gate here, and the socket, which discards a frame whenever the
  // WebSocket is not OPEN (every reconnect passes through that state). A
  // swallowed send booked as sent is worse than no send at all, because the
  // dedupe then suppresses the re-assert forever and the child keeps drawing
  // for a viewport nobody is looking at. What the server DOES with a frame it
  // received is its own business, so this records "written to the socket" and
  // claims nothing more.
  //
  // A steady-state resize by the current owner does NOT change the owner (no
  // `pty.owner` echo), so it deliberately does not arm a handover; only an
  // ownership-ACQUIRING claim does. Every claim now runs with the verdict
  // ALREADY flipped to "mine" (take-over flips it before bouncing the socket;
  // the freed-pty claim flips it before sending), so claims pass this gate and
  // are recorded like any other send. That is a change from the shape this
  // comment used to describe, where a claim ran while the verdict still said
  // somebody else owned the pty and had to bypass the record entirely.
  //
  // What has NOT changed is the direction of the surviving error: a resize the
  // server refuses is still booked here as sent (this records "written to the
  // socket" and claims nothing about what the server did with it), so the next
  // size check may skip a frame the PTY never actually got. The foreground
  // resync's forced re-send is the standing recovery, and a same-size frame is
  // a kernel no-op. A non-owner cannot reach here at all, so the only refusable
  // frame is one whose ownership was lost between the gate and the wire.
  const sendOwned = (rows: number, cols: number): boolean => {
    if (!isOwner()) return false
    if (!sendResize(rows, cols)) return false
    lastRows = rows
    lastCols = cols
    return true
  }

  const sendSize = () => {
    // Never land a SIGWINCH inside an active touch-scroll's wheel-report
    // stream: hold the send and let the lift flush it after the finger goes.
    if (holding) {
      resizeHeldByGesture = true
      return
    }
    if (term.rows !== lastRows || term.cols !== lastCols) {
      sendOwned(term.rows, term.cols)
    }
  }

  const armDebounce = () => {
    clearTimeout(sendTimer)
    sendTimer = setTimeout(sendSize, RESIZE_SEND_DEBOUNCE_MS)
  }

  // The ResizeObserver's local refit: do it now, or mark it held for the
  // gesture-end flush. Never fit while the pair is held.
  const fitOrHold = () => {
    if (holding) {
      fitHeldByGesture = true
      return
    }
    fit.fit()
  }

  // A direct resize request (the first-frame jiggle, the reconnect resize, the
  // freed-pty claim, the foreground resync): refit and notify the
  // child now, or defer BOTH halves to gesture end. These paths bypass the
  // debounce on purpose, so each has to route through the hold explicitly or
  // the pair comes apart again.
  const fitAndSend = (send: () => void) => {
    if (holding) {
      fitHeldByGesture = true
      // FIRST one wins while held: plain resize sends are interchangeable
      // (each re-reads the live geometry when it finally runs), but the
      // first-open jiggle closure is not, and a later plain resize overwriting
      // a parked jiggle would silently skip the redraw nudge for that open
      // (`initialResizeDone` is already latched by then).
      heldResizeSend = heldResizeSend ?? send
      return
    }
    fit.fit()
    send()
  }

  // Defer the initial PTY resize until the FIRST PTY frame after each (re)open
  // has fully rendered. That frame is the server's repaint: a STATIC snapshot
  // taken at the PTY's current size, which can differ from this viewport.
  // Resizing too early (before the repaint has arrived, or mid-render) races a
  // half-painted buffer and leaves the cursor and the bottom-anchored agent
  // prompt in the wrong rows; only a later real resize fixed it. xterm's write
  // callback fires once that frame is parsed, so the fit + resize happens right
  // after it lands and the agent's SIGWINCH redraw cleanly replaces the
  // snapshot at the true size.
  let initialResizeDone = false
  // Whether the NEXT first-frame resize should jiggle (very first open) or send
  // a single plain resize (a reconnect). `noteOpen` sets it before the first
  // frame lands; it defaults to `true` so the very first open still jiggles
  // even in the pathological case where the fallback timer beats the open.
  let firstFrameIsFirstOpen = true

  const firstFrameLanded = () => {
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
        // FIRST open only: force the agent to FULLY redraw at our size now that
        // the first paint has landed. A same-size resize is a kernel no-op (no
        // SIGWINCH), so when the PTY already matches this viewport the agent
        // never repaints and the initial snapshot (imperfect for a tall buffer
        // with a bottom-anchored prompt) stays on screen with the cursor and
        // input box misplaced. Nudge the width down one column and back: each
        // step is a real winsize change, so the kernel raises SIGWINCH and the
        // agent redraws its true UI, ending at the correct size. This automates
        // the manual divider-nudge that reliably fixed it.
        sendOwned(term.rows, Math.max(1, term.cols - 1))
        jiggleTimer = setTimeout(() => {
          // The continuation is its own direct send, so it takes the same hold:
          // a gesture that started inside the window would otherwise catch this
          // SIGWINCH mid-stream.
          fitAndSend(() => sendOwned(term.rows, term.cols))
        }, JIGGLE_STEP_MS)
      } else {
        // RECONNECT: the server kept the PTY alive at its prior size and
        // replays a fresh repaint as this first frame. Jiggling here would
        // force TWO full-screen agent repaints (at two widths) on EVERY
        // reconnect, and mobile reconnects constantly. Send a SINGLE resize to
        // our true size instead: it still re-asserts ownership, it is a kernel
        // no-op (no repaint) when the size is unchanged, and it raises exactly
        // one natural SIGWINCH only when the viewport genuinely changed while
        // disconnected.
        sendOwned(term.rows, term.cols)
      }
    })
  }

  return {
    fitAfterOpen() {
      fit.fit()
    },
    start(container) {
      // Geometry is reported to the PTY from exactly one place: xterm's own
      // resize event. A local re-grid has more causes than the ResizeObserver,
      // and every one of them has to reach the child or it draws for a geometry
      // the browser is not rendering. The case that shipped broken is the
      // font-load refit: the bundled faces arrive after the terminal is already
      // open, the cell metrics move, the terminal re-grids with no container
      // resize anywhere, and nothing was watching, so the PTY kept the size the
      // fallback metrics produced. On a phone that left a copy of the agent's
      // cursor-relative status line behind on every redraw. Be precise about
      // what did and did not heal: the SIZE MISMATCH fixed itself at the next
      // container resize (the dedupe still held the pre-font values, so that
      // fit sent); what never healed is the duplicated output already written
      // into the scrollback. Subscribing here covers that cause and any future
      // one, instead of teaching each call site to report. xterm fires this
      // only when the grid really changed, and the debounce plus the dedupe
      // keep a no-op fit off the wire.
      resizeSub = term.onResize(() => armDebounce())
      // Local fit so the canvas matches this viewport right away, and seed the
      // dedupe so the ResizeObserver's initial observe callback does NOT send a
      // (racing) resize before the first paint. The initial PTY resize is
      // deferred to the first-frame handler.
      fit.fit()
      lastRows = term.rows
      lastCols = term.cols
      // Fallback for a session that emits no first frame (e.g. an idle freshly
      // launched agent): size its PTY anyway. If the first frame arrives first,
      // the `initialResizeDone` guard makes this a no-op.
      initialResizeFallback = setTimeout(
        firstFrameLanded,
        INITIAL_RESIZE_FALLBACK_MS,
      )
      // (A background tab throttles rAF but not timers, so a resize received
      // while hidden refits late or not at all and its debounced send dedupes
      // to a no-op; the foreground resync is the designed recovery.)
      ro = new ResizeObserver(() => {
        cancelAnimationFrame(fitFrame)
        // Through the hold, never a bare fit: a refit landing mid-touch-gesture
        // resets the child's scrolling region under it.
        fitFrame = requestAnimationFrame(() => fitOrHold())
        armDebounce()
      })
      ro.observe(container)
    },
    noteOpen(firstOpen) {
      initialResizeDone = false
      // A reconnect must NOT jiggle: an unchanged size would double-repaint the
      // agent on every mobile reconnect.
      firstFrameIsFirstOpen = firstOpen
    },
    needsFirstFrameResize: () => !initialResizeDone,
    firstFrameLanded,
    directSend: fitAndSend,
    resyncToForeground() {
      // Debounced (coalescing rapid focus/visibility flaps) and gated on xterm
      // draining its write queue: a foreground return can coincide with the
      // server's scrollback replay still streaming in, and resizing mid-replay
      // corrupts the scroll position. The empty-write callback fires only once
      // the queued writes have drained, so the fit lands against a settled
      // buffer. The send is FORCED (not routed through the deduped `sendSize`)
      // because the PTY's current size may have been set by ANOTHER client, so
      // the cached record would wrongly suppress the re-assert; a same-size
      // resize is a kernel no-op, so re-asserting costs nothing.
      clearTimeout(resyncTimer)
      resyncTimer = setTimeout(() => {
        term.write("", () => {
          // The pair again: a foreground return that lands mid-gesture defers
          // both halves to the lift rather than refitting under the finger.
          fitAndSend(() => sendOwned(term.rows, term.cols))
        })
      }, FOREGROUND_RESYNC_MS)
    },
    setHolding(next) {
      holding = next
    },
    flushHeld() {
      // Release the resize pair the gesture held back: the local refit runs
      // exactly once, at the final container size, and the child's notification
      // follows it immediately. Exactly one fit, whichever halves were held: a
      // direct-send path does not fit for itself while held, precisely so this
      // flush cannot double-fit.
      const pendingSend = heldResizeSend
      heldResizeSend = null
      if (fitHeldByGesture) {
        fitHeldByGesture = false
        fit.fit()
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
