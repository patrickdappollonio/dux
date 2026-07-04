import type { ConnState } from "./types"

// The single home for the frontend's WebSocket reconnect behavior. Both the
// `/ws/events` JSON spine socket (`EventsSocket`) and every per-PTY byte socket
// (`PtySocket`) extend this base so the lifecycle — capped-exponential backoff,
// the hard attempt cap, `ConnState` emission, the `closedByUser` short-circuit,
// and the orphan/identity guards against a double `connect()` — lives in exactly
// one place and can never drift between the two connections.
//
// Backoff: retry with exponential backoff from `RECONNECT_MIN_MS`, doubling each
// attempt up to `RECONNECT_MAX_MS`. After `MAX_RECONNECT_ATTEMPTS` consecutive
// failures the socket gives up, emits `failed`, and STOPS — a genuinely-down
// server hands control back to the user (a Reconnect affordance) rather than
// spinning forever. A deliberate `connect()` resets the whole budget.
export const RECONNECT_MIN_MS = 500
export const RECONNECT_MAX_MS = 5000

// How many consecutive reconnect attempts before the socket gives up and signals
// `failed`. Kept deliberately small (a handful of tries) so a genuinely-down
// server surfaces a Reconnect affordance quickly rather than retrying silently
// and indefinitely. Shared by BOTH sockets so the app-wide offline modal (driven
// by the events socket) and a focused terminal give up on the same schedule —
// the old asymmetry (uncapped PTY silently reattaching behind a stuck overlay)
// is impossible by construction now.
export const MAX_RECONNECT_ATTEMPTS = 3

// The shared reconnecting WebSocket base. Subclasses supply the socket-specific
// bits through the protected hooks below; everything to do with *when* to
// reconnect and *what connection state to emit* is owned here.
export abstract class ReconnectingSocket {
  protected url: string
  protected ws: WebSocket | null = null
  private reconnectDelay = RECONNECT_MIN_MS
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  protected closedByUser = false
  private attempts = 0

  // Connection-state transitions ("connecting" | "open" | "closed" | "failed").
  // Drives the status indicator / offline modal (events socket) and the focused
  // terminal's give-up affordance (PTY socket).
  onConn: (state: ConnState) => void = () => {}
  // Fired after the socket (re)opens AND the subclass's `onSocketOpen` hook has
  // run (EventsSocket resends its subscription set; PtySocket has nothing to
  // resend). Lets the consumer re-fetch / re-arm after every open.
  onOpen: () => void = () => {}
  // Fired once per drop, when a reconnect is actually scheduled (NOT on the
  // give-up attempt and NOT on a user-initiated `close()`). Lets a consumer show
  // a non-blocking "Reconnecting…" cue. EventsSocket leaves this a no-op.
  onReconnecting: () => void = () => {}

  constructor(url: string) {
    this.url = url
  }

  // A deliberate, user-initiated (re)entry: reset the reconnect bookkeeping
  // (attempts + backoff + closedByUser) so a fresh connect never inherits an
  // exhausted counter from a prior session. This is also the manual "Reconnect"
  // path: calling it on a socket that already gave up (`failed`) restarts the
  // loop cleanly.
  connect(): void {
    this.closedByUser = false
    this.attempts = 0
    this.reconnectDelay = RECONNECT_MIN_MS
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.open()
  }

  private open(): void {
    // A socket may already be live here: a double connect() (double-click
    // Reconnect firing connect() mid-reconnect) would otherwise overwrite
    // `this.ws` and leave an orphan whose later `onclose` nulls the SHARED
    // `this.ws` field — permanently killing outbound frames (stale changes pane /
    // frozen terminal, no error). Detach the orphan's handlers and close it
    // BEFORE assigning the new socket so it can never run a handler against
    // shared state again.
    if (this.ws !== null) {
      const orphan = this.ws
      orphan.onopen = null
      orphan.onmessage = null
      orphan.onclose = null
      orphan.onerror = null
      this.ws = null
      orphan.close()
    }
    this.onConn("connecting")
    const ws = new WebSocket(this.url)
    this.configureSocket(ws)
    this.ws = ws

    ws.onopen = () => {
      // Identity guard: only the socket that is still the live `this.ws` may
      // mutate shared connection state. A late callback from a socket a newer
      // open() already replaced must be inert.
      if (this.ws !== ws) return
      // A successful open means the connection is usable again, so refill the
      // retry budget: the next drop starts a fresh retry schedule. A definitive
      // failure where the socket opens but is not usable — a PTY whose provider
      // failed to launch or has exited — is signalled by an explicit server close
      // code and handled in `onclose`/`shouldReconnect`, not by withholding this.
      this.markHealthy()
      this.onSocketOpen()
      this.onConn("open")
      this.onOpen()
    }

    ws.onmessage = (event) => {
      if (this.ws !== ws) return
      this.handleMessage(event)
    }

    ws.onclose = (event) => {
      // Only the live socket nulls the shared ref and drives reconnect. Without
      // this identity check an orphan's close would null the live `this.ws`,
      // silently dropping every later outbound frame.
      if (this.ws !== ws) return
      this.ws = null
      this.onConn("closed")
      if (this.closedByUser) return
      // The close carries a code. A server may close with an app-specific code to
      // say "do not retry" — e.g. a PTY whose provider failed to launch or has
      // exited, where re-subscribing would just relaunch the doomed provider.
      // shouldReconnect() inspects the code and, for such a terminal close,
      // surfaces the stop state and returns false. Any other close (a transient
      // transport drop, typically code 1006) retries up to the shared cap.
      if (!this.shouldReconnect(event.code)) return
      this.scheduleReconnect()
    }

    // `onerror` is followed by `onclose`; let the close handler drive reconnect.
    ws.onerror = (event) => {
      this.handleError(event)
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return
    // Retry with capped exponential backoff, but after MAX_RECONNECT_ATTEMPTS
    // give up and signal `failed` so the UI shows a deliberate Reconnect
    // affordance. A manual `connect()` resets the counter and resumes.
    this.attempts++
    if (this.attempts > MAX_RECONNECT_ATTEMPTS) {
      this.onConn("failed")
      return
    }
    // We are about to retry (not give up): signal the consumer so it can show a
    // non-blocking "Reconnecting…" state. Fired once per drop (the timer guard
    // above keeps a single retry in flight).
    this.onReconnecting()
    const delay = this.reconnectDelay
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (!this.closedByUser) {
        this.open()
      }
    }, delay)
  }

  close(): void {
    this.closedByUser = true
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.ws?.close()
  }

  // Refill the reconnect budget (attempt count + backoff) — call when the
  // connection is confirmed usable so the next drop starts a fresh retry
  // schedule from the minimum delay. The base calls this from `onopen` for
  // sockets whose open proves usability; a subclass whose open does not (the PTY
  // socket) calls it directly from its own readiness signal instead.
  protected markHealthy(): void {
    this.reconnectDelay = RECONNECT_MIN_MS
    this.attempts = 0
  }

  // ---- Subclass extension hooks ----------------------------------------------

  // Tweak the freshly-constructed WebSocket before handlers are attached (e.g.
  // PtySocket sets `binaryType = "arraybuffer"`). Default: no-op. (`void ws` keeps
  // the param in the base signature — the base calls this with the new socket —
  // without tripping no-unused-vars.)
  protected configureSocket(ws: WebSocket): void {
    void ws
  }

  // Run subclass-specific work on every (re)open, BEFORE `onOpen` fires:
  // EventsSocket re-sends its chunked subscription set here; PtySocket has
  // nothing to resend (the server replays scrollback as the first frame).
  protected abstract onSocketOpen(): void

  // Handle one server frame. EventsSocket parses text; PtySocket splits binary
  // (PTY bytes) from the text `connected` handshake.
  protected abstract handleMessage(event: MessageEvent): void

  // Consulted on every unexpected close, before scheduling a reconnect, with the
  // close code. Returning `false` stops the loop for good (PtySocket uses it for a
  // deleted extra tab's now-gone route and for a server "provider unavailable"
  // close code). Default: always reconnect, whatever the code.
  protected shouldReconnect(closeCode: number): boolean {
    void closeCode
    return true
  }

  // React to the socket's `error` event. Default: no-op (EventsSocket); PtySocket
  // logs a breadcrumb. (`void event` keeps the param without tripping the
  // unused-vars lint.)
  protected handleError(event: Event): void {
    void event
  }
}
