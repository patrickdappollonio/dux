// Per-PTY active-owner model (client side). A PTY (an agent's provider or a
// companion terminal) is SHARED across every connected device, but only ONE of
// them — the "owner" — drives its size and may type into it. The others render a
// read-only take-over placeholder. This mirrors the server's `PtySizeOwners`
// (`crates/dux-web/src/server.rs`): a connection claims ownership by sending a
// size frame (most-recent claim wins), and the server broadcasts a `pty.owner`
// signal on every real handover.
//
// This module holds the small, pure pieces of that model so they are unit-
// testable without rendering xterm (which needs a DOM/canvas harness the web
// tests deliberately avoid): the foreground check that decides whether a fresh
// attach claims, the self-claim grace window that distinguishes our own
// `pty.owner` echo from another device taking over, and the `pty.owner` event
// fan-out the store pushes into.

// A foregrounded tab claims ownership on attach by sending its size; a
// backgrounded tab attaches as a silent observer and sends nothing. This is the
// decision the terminal view seeds its initial owner state from (and gates its
// size sends on). Read at call time so tests can stub `document.visibilityState`
// per case, and treated as foreground when there is no `document` (non-DOM
// contexts) so a claim is never silently suppressed.
export function isForeground(): boolean {
  return typeof document === "undefined"
    ? true
    : document.visibilityState === "visible"
}

// An OWNERSHIP-ACQUIRING claim (a fresh attach claim or a take-over) changes the
// owner server-side, so the server broadcasts ONE `pty.owner` for our pty to
// every client — including us. We must recognise that echo as our own rather than
// reading it as another device taking control. We do so by counting: each
// acquiring claim arms exactly one expected echo, consumed by the next matching
// `pty.owner`. (Steady-state resizes by the current owner do NOT change the owner,
// so they emit no echo and must NOT arm one — see TerminalPane: only the attach
// claim and take-over note a claim.) Events arrive in broadcast order, so our own
// echo (emitted at our claim) precedes a later takeover by another device, and the
// count resolves the two correctly even when they fall close together.
//
// The grace window is a safety backstop only: if an armed echo never arrives (a
// dropped/lagged event), it is not allowed to absorb a much-later genuine
// takeover — past the window a `pty.owner` is always read as a demotion.
export const SELF_CLAIM_GRACE_MS = 1500

// Tracks whether an incoming `pty.owner` for OUR pty is our own claim echoing back
// or another device taking over. `now` is injectable so tests advance time
// deterministically.
export class PtyOwnershipTracker {
  private pendingEchoes = 0
  private lastClaimAt = Number.NEGATIVE_INFINITY
  private readonly now: () => number

  constructor(now: () => number = Date.now) {
    this.now = now
  }

  // Record that THIS view just made an ownership-acquiring claim (sent a size
  // that takes the PTY), arming one expected `pty.owner` echo.
  noteLocalClaim(): void {
    this.pendingEchoes += 1
    this.lastClaimAt = this.now()
  }

  // A `pty.owner` event for OUR pty just arrived: whether it means we were demoted
  // (another device claimed). False — and one armed echo consumed — when it is our
  // own recent claim echoing back; true otherwise. A stale armed echo (past the
  // grace window) is discarded so it can never absorb a genuine later takeover.
  isDemotion(): boolean {
    if (this.pendingEchoes > 0 && this.now() - this.lastClaimAt < SELF_CLAIM_GRACE_MS) {
      this.pendingEchoes -= 1
      return false
    }
    this.pendingEchoes = 0
    return true
  }
}

// `pty.owner` fan-out. The store's single `/ws/events` handler receives the
// signal and calls `notifyPtyOwner(ptyId)`; each mounted terminal view registers
// an `onPtyOwner` listener and reacts only to its own pty id. Kept here (not in
// the store) so the terminal view depends on a small leaf module rather than the
// store, matching the `setActivePtySocket` singleton pattern in `ptySocket.ts`.
type PtyOwnerListener = (ptyId: string) => void
const ptyOwnerListeners = new Set<PtyOwnerListener>()

export function onPtyOwner(cb: PtyOwnerListener): () => void {
  ptyOwnerListeners.add(cb)
  return () => {
    ptyOwnerListeners.delete(cb)
  }
}

export function notifyPtyOwner(ptyId: string): void {
  // Snapshot so a listener that unsubscribes during dispatch can't perturb the
  // live iteration.
  for (const cb of [...ptyOwnerListeners]) cb(ptyId)
}
