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
// Reconnect behavior is the shared `ReconnectingSocket` base: capped exponential
// backoff WITH the same hard 3-attempt cap the events socket uses. When the
// budget is spent the socket emits `failed` and STOPS (rather than silently
// reattaching behind a stuck offline overlay); the focused pane surfaces a
// Reconnect affordance and a manual reconnect resets the budget. `close()` is the
// deliberate, user-initiated teardown and suppresses the reconnect loop.

import { ReconnectingSocket } from "./reconnectingSocket"

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

// An extra tab's PTY socket URL, nested under its owning session so the server
// can enforce that the tab belongs to that session. Used ONLY for extra tabs;
// the session-slot tab keeps `agentPtyUrl` (its `tab_id === session_id`, served by
// the existing `/ws/sessions/:id/pty` route). Connecting launches the extra
// tab's provider fresh (there is no resume for extra tabs).
export function tabPtyUrl(sessionId: string, tabId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/tabs/${encodeURIComponent(tabId)}/pty`
}

export class PtySocket extends ReconnectingSocket {
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

  // `onOpen`, `onReconnecting`, and `onConn` are inherited from ReconnectingSocket.
  // The pane wires `onOpen` (re-arm first-frame resize; the server replays
  // scrollback as the first Binary frame on every open), `onReconnecting` (show a
  // non-blocking "Reconnecting…" cue while the socket retries), and `onConn` (so
  // it can surface a "connection lost" Reconnect affordance when the shared cap is
  // hit and the socket emits `failed`). Input typed while disconnected is still
  // dropped by `sendInput`'s readyState guard — the cues signal that it would be,
  // they are not a buffer.

  // Consulted on every unexpected close, BEFORE scheduling a reconnect. Returning
  // `false` means the underlying PTY route is gone for good (e.g. an extra tab's
  // socket 404s because another client deleted that tab while this one was
  // retrying) rather than merely dropped — retrying against a route that will keep
  // 404ing would spin with no escape. A close carries no HTTP status the client
  // can read, so the consumer is expected to check its own source of truth (e.g.
  // spine tab membership) instead. Defaults to always retry, matching every other
  // PTY socket (agent session-slot tab, companion terminal), which never go away
  // out from under a live client this way.
  shouldRetry: () => boolean = () => true

  // Fired once, in place of scheduling a reconnect, the first time `shouldRetry()`
  // says the route is gone. The socket does not close itself further (there is
  // nothing more to tear down); the consumer decides what the UI does next.
  onGone: () => void = () => {}

  // Register the raw-bytes consumer (xterm `term.write`). Last registration wins.
  onBytes(cb: (bytes: Uint8Array) => void): void {
    this.bytesCb = cb
  }

  // This socket's current connection id, or null before the `connected` frame.
  get connectionId(): string | null {
    return this.connId
  }

  // Request arraybuffer framing so server→client Binary frames arrive as
  // `ArrayBuffer` (raw PTY bytes) rather than Blobs.
  protected configureSocket(ws: WebSocket): void {
    ws.binaryType = "arraybuffer"
  }

  // Nothing to resend on (re)open: the server replays this PTY's scrollback as the
  // first Binary frame after every open, so the byte feed rehydrates itself.
  protected onSocketOpen(): void {}

  protected handleMessage(event: MessageEvent): void {
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

  // A gone route is a hard stop, not a transient drop: fire `onGone` once and tell
  // the base not to schedule a reconnect. Every other PTY drop reconnects normally
  // (the default) up to the shared attempt cap.
  protected shouldReconnect(): boolean {
    if (!this.shouldRetry()) {
      this.onGone()
      return false
    }
    return true
  }

  // Warn so a flapping PTY socket leaves a console breadcrumb instead of failing
  // silently; the visible reconnect signal is driven by `onReconnecting`.
  protected handleError(event: Event): void {
    console.warn("[dux] PTY socket error; reconnect will follow", event)
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
