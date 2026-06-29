// A dedicated WebSocket to ONE PTY (an agent's main provider or a companion
// terminal), introduced in Phase 5. It replaces the legacy `DuxSocket`
// Subscribe/SubscribeTerminal/Resize/binary-frame multiplexing: each focused
// terminal opens its own socket whose connection IS the subscription, so the
// server routes that PTY's bytes here with no per-message addressing.
//
// Protocol (matches `handle_pty_socket` in `crates/dux-web/src/server.rs`):
//   - On (re)open the server sends a Text `connected` frame FIRST carrying this
//     socket's connection id: `{"event":"connected","id":"<connId>"}`.
//   - Then the server sends ONE Binary frame replaying the buffered
//     scrollback/repaint; feed it straight to xterm like any other byte chunk.
//   - server→client Binary = raw PTY bytes (write to xterm).
//   - client→server Binary = PTY stdin (xterm `onData`).
//   - client→server Text = a resize control frame `{"rows":R,"cols":C}`.
//   - Close = detach (the server drops the subscription/forwarder).
//
// Reconnect mirrors `EventsSocket`: capped exponential backoff with NO hard
// attempt cap (a PTY whose socket dropped should keep trying until the user
// navigates away). `close()` is the deliberate, user-initiated teardown and
// suppresses the reconnect loop.

const RECONNECT_MIN_MS = 500
const RECONNECT_MAX_MS = 5000

// Derive the WebSocket scheme from the page protocol so an HTTPS deployment uses
// `wss://` (a hardcoded `ws://` would be blocked as mixed content under HTTPS).
// Read at call time (not module load) so the URL builders are safe to import in
// any environment and tests can stub `location` per-case.
function wsScheme(): string {
  return location.protocol === "https:" ? "wss:" : "ws:"
}

// The agent session's main PTY socket URL. Connecting launches/resumes the
// provider, exactly as the legacy `Subscribe` did.
export function agentPtyUrl(sessionId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/pty`
}

// A companion terminal's PTY socket URL, nested under its owning session so the
// server can enforce that the terminal belongs to that session.
export function terminalPtyUrl(sessionId: string, terminalId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/terminals/${encodeURIComponent(terminalId)}/pty`
}

export class PtySocket {
  private url: string
  private ws: WebSocket | null = null
  private reconnectDelay = RECONNECT_MIN_MS
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closedByUser = false
  private bytesCb: (bytes: Uint8Array) => void = () => {}
  // This socket's server-assigned connection id, delivered as the first Text frame
  // (`{event:"connected", id}`) on every (re)open (the server allocates a fresh id
  // per open). Null until that frame arrives. The terminal view compares it against
  // the `owner` field of each `pty.owner` event to decide ownership definitively
  // (see `ptyOwnership.ts`).
  private connId: string | null = null

  // Fired with this socket's connection id each time the `connected` frame lands.
  // Lets the terminal view track which connection id is "us" for the ownership
  // comparison, re-issued on every reconnect.
  onConnected: (id: string) => void = () => {}

  // Fired after each (re)open. The server replays scrollback as the first Binary
  // frame on every open, so the consumer uses this to re-arm its first-frame
  // resize handling (the reconnect repaint must be re-sized like the initial one).
  onOpen: () => void = () => {}

  // Fired when an unexpected drop schedules a reconnect (NOT on a user-initiated
  // `close()`). Lets the consumer surface a non-blocking "Reconnecting…" state and
  // re-arm its readiness spinner while the socket is down. Pairs with `onOpen`,
  // which signals the socket is live again. Input typed while disconnected is
  // still dropped by `sendInput`'s readyState guard — this is the user-facing
  // signal that it would be, not a buffer.
  onReconnecting: () => void = () => {}

  constructor(url: string) {
    this.url = url
  }

  // Register the raw-bytes consumer (xterm `term.write`). Last registration wins.
  onBytes(cb: (bytes: Uint8Array) => void): void {
    this.bytesCb = cb
  }

  // This socket's current connection id, or null before the `connected` frame.
  get connectionId(): string | null {
    return this.connId
  }

  // A deliberate, user-initiated (re)entry: reset the reconnect bookkeeping so a
  // fresh connect never inherits a stale backoff. Mirrors `EventsSocket.connect`.
  connect(): void {
    this.closedByUser = false
    this.reconnectDelay = RECONNECT_MIN_MS
    this.open()
  }

  private open(): void {
    const ws = new WebSocket(this.url)
    ws.binaryType = "arraybuffer"
    this.ws = ws

    ws.onopen = () => {
      this.reconnectDelay = RECONNECT_MIN_MS
      this.onOpen()
    }

    ws.onmessage = (event) => {
      // Binary frames carry PTY bytes (the scrollback replay arrives as an ordinary
      // Binary frame too). The ONLY Text frame the server sends is the opening
      // `connected` handshake carrying this socket's connection id; record it for
      // the ownership comparison and notify the consumer.
      if (event.data instanceof ArrayBuffer) {
        this.bytesCb(new Uint8Array(event.data))
        return
      }
      if (typeof event.data === "string") {
        try {
          const frame = JSON.parse(event.data) as { event?: string; id?: string }
          if (frame.event === "connected" && typeof frame.id === "string") {
            this.connId = frame.id
            this.onConnected(frame.id)
          }
        } catch {
          // A malformed control frame is not fatal to the byte stream; ignore it.
        }
      }
    }

    ws.onclose = () => {
      this.ws = null
      if (!this.closedByUser) {
        this.scheduleReconnect()
      }
    }

    // `onerror` is followed by `onclose`; let the close handler drive reconnect.
    // Warn so a flapping PTY socket leaves a console breadcrumb instead of failing
    // silently; the visible reconnect signal is driven by `onReconnecting`.
    ws.onerror = (event) => {
      console.warn("[dux] PTY socket error; reconnect will follow", event)
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return
    // The socket dropped and we are about to retry: signal the consumer so it can
    // show a non-blocking "Reconnecting…" state. Fired once per drop (the
    // reconnectTimer guard above keeps a single retry in flight).
    this.onReconnecting()
    // No attempt cap (mirrors `EventsSocket`): a focused PTY whose socket dropped
    // keeps retrying with capped backoff until the user navigates away (which
    // calls `close()`). Giving up would silently freeze the terminal.
    const delay = this.reconnectDelay
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (!this.closedByUser) {
        this.open()
      }
    }, delay)
  }

  // Send PTY stdin as a Binary frame. A copy is sent so the buffer is a plain
  // `ArrayBuffer` (not `ArrayBufferLike`, which `WebSocket.send` rejects under
  // strict lib typings) and the caller's view can't mutate it in flight.
  sendInput(bytes: Uint8Array): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(bytes.slice().buffer)
    }
  }

  // Send a resize control frame as Text. The server parses `{rows, cols}` (u16)
  // and issues the SIGWINCH; an unchanged size is a kernel no-op server-side.
  sendResize(rows: number, cols: number): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ rows, cols }))
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

// The PTY socket the focused center pane is currently driving, or null when no
// terminal is focused. The macro quick-picker writes a macro's payload straight
// to this socket as stdin (Phase 5 dropped the server-side `run_macro` command),
// so the store needs a handle to "the active PTY" without reaching into React.
// `TerminalPane` registers its socket on mount and clears it on unmount.
let activePtySocket: PtySocket | null = null

export function setActivePtySocket(s: PtySocket | null): void {
  activePtySocket = s
}

export function getActivePtySocket(): PtySocket | null {
  return activePtySocket
}
