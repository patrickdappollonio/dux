import type {
  ClientMessage,
  CommandStatus,
  ConnState,
  ServerMessage,
  ViewModel,
} from "./types"

const RECONNECT_MIN_MS = 500
const RECONNECT_MAX_MS = 5000

// DuxSocket wraps a single WebSocket connection to the dux web server. It
// dispatches parsed server frames to consumer-settable callbacks and exposes
// helpers for the client -> server protocol. The connection auto-reconnects
// with simple exponential backoff and re-emits connection state changes.
export class DuxSocket {
  private url: string
  private ws: WebSocket | null = null
  private reconnectDelay = RECONNECT_MIN_MS
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closedByUser = false

  // Consumer callbacks. Defaults are no-ops so consumers only wire what they need.
  onViewModel: (vm: ViewModel) => void = () => {}
  onCommandResult: (status: CommandStatus | null, error: string | null) => void =
    () => {}
  onError: (message: string) => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  onPtyBytes: (bytes: Uint8Array) => void = () => {}

  constructor(url: string) {
    this.url = url
  }

  connect(): void {
    this.closedByUser = false
    this.open()
  }

  private open(): void {
    this.onConn("connecting")
    const ws = new WebSocket(this.url)
    ws.binaryType = "arraybuffer"
    this.ws = ws

    ws.onopen = () => {
      this.reconnectDelay = RECONNECT_MIN_MS
      this.onConn("open")
    }

    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        this.handleText(event.data)
      } else if (event.data instanceof ArrayBuffer) {
        this.onPtyBytes(new Uint8Array(event.data))
      }
    }

    ws.onclose = () => {
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
    let message: ServerMessage
    try {
      message = JSON.parse(raw) as ServerMessage
    } catch {
      return
    }
    switch (message.type) {
      case "view_model":
        this.onViewModel(message.data)
        break
      case "command_result":
        this.onCommandResult(message.status, message.error)
        break
      case "subscribed":
        break
      case "error":
        this.onError(message.message)
        break
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return
    const delay = this.reconnectDelay
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (!this.closedByUser) {
        this.open()
      }
    }, delay)
  }

  private sendJson(message: ClientMessage): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message))
    }
  }

  sendCommand(command: string, args: Record<string, unknown>): void {
    this.sendJson({ type: "command", command, args })
  }

  subscribe(sessionId: string): void {
    this.sendJson({ type: "subscribe", session_id: sessionId })
  }

  resize(sessionId: string, rows: number, cols: number): void {
    this.sendJson({ type: "resize", session_id: sessionId, rows, cols })
  }

  sendInput(bytes: Uint8Array): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      // Send a copy so the buffer is a plain `ArrayBuffer` (not `ArrayBufferLike`,
      // which `WebSocket.send` rejects under strict lib typings).
      this.ws.send(bytes.slice().buffer)
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
