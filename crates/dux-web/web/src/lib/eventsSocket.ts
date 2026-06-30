import type {
  ConnState,
  EventsClientMessage,
  EventsServerMessage,
} from "./types"

// Backoff mirrors the PTY socket (`ptySocket.ts`) so the two connections behave
// identically under flaky networks.
const RECONNECT_MIN_MS = 500
const RECONNECT_MAX_MS = 5000

// How many consecutive reconnect attempts before the socket gives up and signals
// `failed` (the bottom-left "Connection failed" indicator + Reconnect button, and
// the app-wide offline modal's "gave up" state). Since Phase 6 this is the ONLY
// JSON socket, so it owns the connection-state UX the retired `DuxSocket` used to:
// a normal blip auto-recovers within a few backoff-spaced attempts; exhausting
// them surfaces a deliberate Reconnect affordance (and, when authed, triggers the
// auth recheck — an expired session 401s the gated upgrade so the loop can never
// open). Kept deliberately small (a handful of tries) so a genuinely-down server
// hands control back to the user quickly rather than spinning indefinitely.
const MAX_RECONNECT_ATTEMPTS = 3

// The server silently rejects an interest frame carrying more than this many
// topics (its `MAX_EVENT_TOPICS_PER_FRAME`). The reconnect resend, which sends
// the WHOLE interest set, chunks into frames of at most this size so a client
// watching many sessions never loses its tail of subscriptions on reconnect.
const MAX_EVENT_TOPICS_PER_FRAME = 64

// EventsSocket wraps the `/ws/events` channel — since Phase 6 the ONLY JSON
// socket (the legacy `/ws`/`DuxSocket` is gone; PTY bytes ride their own per-PTY
// sockets in `lib/ptySocket.ts`). It (a) maintains the full set of topics this
// client is interested in, (b) forwards every server frame (resource-change
// events plus the `connected`/`status`/`status_cleared` control frames the old
// `/ws` used to carry) to a consumer callback, and (c) re-emits its connection
// state so the store can drive the indicator + auth recovery.
//
// The client sends `{ "subscribe": [...] }` / `{ "unsubscribe": [...] }`; the
// server pushes `{ "event": "session.changes", "id": "<id>", "rev": <n> }` and
// friends. The whole subscription set is re-sent on every (re)open so a dropped
// connection loses no interest. After re-subscribing, `onOpen` lets the store
// re-fetch the restored topics (a missed event during the outage is recovered
// that way).
export class EventsSocket {
  private url: string
  private ws: WebSocket | null = null
  private reconnectDelay = RECONNECT_MIN_MS
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closedByUser = false
  private attempts = 0
  // The complete, authoritative interest set. Coarse topics (sessions/projects/
  // config) plus the per-screen fine topics (session:<id>:changes) all live
  // here, so a reconnect can re-send the lot.
  private readonly subscriptions = new Set<string>()

  // Consumer callbacks. Defaults are no-ops so the store only wires what it needs.
  onEvent: (ev: EventsServerMessage) => void = () => {}
  // Fired after the socket (re)opens AND the full subscription set has been
  // re-sent, so the store can trigger a GET for each restored topic.
  onOpen: () => void = () => {}
  // Connection-state transitions ("connecting" | "open" | "closed" | "failed"),
  // driving the status-bar indicator and (on "failed" while authed) the auth
  // recheck. Replaces the retired `DuxSocket.onConn`.
  onConn: (state: ConnState) => void = () => {}

  constructor(url: string) {
    this.url = url
  }

  // The current interest set (sorted for deterministic test assertions). Read
  // only — mutate via subscribe/unsubscribe.
  get topics(): string[] {
    return [...this.subscriptions].sort()
  }

  // A deliberate, user-initiated (re)entry: reset the reconnect bookkeeping
  // (attempts + backoff) so a fresh connect never inherits an exhausted counter
  // from a prior session. This is also the manual "Reconnect" path: calling it on
  // a socket that already gave up (`failed`) restarts the loop cleanly.
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
    // `this.ws` and leave an orphan whose later
    // `onclose` nulls the SHARED `this.ws` field — permanently killing outbound
    // subscribe/unsubscribe (stale changes pane, no error). Detach the orphan's
    // handlers and close it BEFORE assigning the new socket so it can never run
    // a handler against shared state again.
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
    this.ws = ws

    ws.onopen = () => {
      // Identity guard: only the socket that is still the live `this.ws` may
      // mutate shared connection state. A late callback from a socket a newer
      // open() already replaced must be inert.
      if (this.ws !== ws) return
      this.reconnectDelay = RECONNECT_MIN_MS
      this.attempts = 0
      // Re-send the WHOLE interest set: server-side interest is per-connection
      // and a dropped connection discards it. Chunked so a large set never
      // exceeds the server's per-frame topic cap (which it would silently drop).
      if (this.subscriptions.size > 0) {
        const topics = [...this.subscriptions]
        for (let i = 0; i < topics.length; i += MAX_EVENT_TOPICS_PER_FRAME) {
          this.sendRaw({
            subscribe: topics.slice(i, i + MAX_EVENT_TOPICS_PER_FRAME),
          })
        }
      }
      this.onConn("open")
      this.onOpen()
    }

    ws.onmessage = (event) => {
      if (this.ws !== ws) return
      if (typeof event.data === "string") {
        this.handleText(event.data)
      }
    }

    ws.onclose = () => {
      // Only the live socket nulls the shared ref and drives reconnect. Without
      // this identity check an orphan's close would null the live `this.ws`,
      // silently dropping every later subscribe/unsubscribe.
      if (this.ws !== ws) return
      this.ws = null
      this.onConn("closed")
      if (!this.closedByUser) {
        this.scheduleReconnect()
      }
    }

    // `onerror` is followed by `onclose`; let the close handler drive reconnect.
    ws.onerror = () => {}
  }

  private handleText(raw: string): void {
    let message: EventsServerMessage
    try {
      message = JSON.parse(raw) as EventsServerMessage
    } catch {
      return
    }
    // Every server frame carries an `event` discriminator — resource-change
    // events (`session.changes`, `projects.changed`, …) plus the control frames
    // the old `/ws` used to carry (`connected`, `status`, `status_cleared`).
    // Forward as-is; the store's single handler
    // switches on `event`. Lag catch-up arrives as an ordinary `session.changes`
    // for this connection, so it is covered too.
    if (typeof message.event === "string") {
      this.onEvent(message)
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return
    // Now the sole JSON socket, EventsSocket owns the connection-state UX: retry
    // with capped exponential backoff, but after MAX_RECONNECT_ATTEMPTS give up
    // and signal `failed` so the UI shows a deliberate Reconnect affordance (and
    // the store rechecks auth when authed — an expired session 401s the upgrade,
    // so the loop can never open on its own). A manual `connect()` resets the
    // counter and resumes.
    this.attempts++
    if (this.attempts > MAX_RECONNECT_ATTEMPTS) {
      this.onConn("failed")
      return
    }
    const delay = this.reconnectDelay
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (!this.closedByUser) {
        this.open()
      }
    }, delay)
  }

  // Add topics to the interest set and (when open) send only the newly-added
  // ones. The full set is re-sent on the next open, so a subscribe issued while
  // the socket is down is not lost.
  subscribe(topics: string[]): void {
    const added: string[] = []
    for (const topic of topics) {
      if (!this.subscriptions.has(topic)) {
        this.subscriptions.add(topic)
        added.push(topic)
      }
    }
    if (added.length > 0) this.sendRaw({ subscribe: added })
  }

  // Remove topics from the interest set and (when open) tell the server. Removing
  // an unknown topic is a no-op.
  unsubscribe(topics: string[]): void {
    const removed: string[] = []
    for (const topic of topics) {
      if (this.subscriptions.delete(topic)) removed.push(topic)
    }
    if (removed.length > 0) this.sendRaw({ unsubscribe: removed })
  }

  private sendRaw(message: EventsClientMessage): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message))
    }
  }

  close(): void {
    this.closedByUser = true
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.ws?.close()
  }
}
