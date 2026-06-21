import type {
  BranchWarningView,
  ClientMessage,
  CommandStatus,
  ConnState,
  DirEntryView,
  ProjectWorktreeEntryView,
  ServerMessage,
  ViewModel,
} from "./types"

const RECONNECT_MIN_MS = 500
const RECONNECT_MAX_MS = 5000
const MAX_RECONNECT_ATTEMPTS = 4

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
  private attempts = 0

  // Consumer callbacks. Defaults are no-ops so consumers only wire what they need.
  onViewModel: (vm: ViewModel) => void = () => {}
  onCommandResult: (status: CommandStatus | null, error: string | null) => void =
    () => {}
  onError: (message: string) => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  onPtyBytes: (bytes: Uint8Array) => void = () => {}
  onTerminalCreated: (sessionId: string, terminalId: string) => void = () => {}
  onStatus: (key: string | null | undefined, tone: string, message: string) => void = () => {}
  onStatusCleared: (key: string | null | undefined) => void = () => {}
  onCommitMessage: (sessionId: string, message: string) => void = () => {}
  onCommitMessageSnapshot: (sessionId: string, message: string) => void =
    () => {}
  onDirEntries: (
    path: string,
    entries: DirEntryView[],
    error: string | null,
  ) => void = () => {}
  onAgentName: (name: string) => void = () => {}
  onProjectWorktrees: (
    projectId: string,
    entries: ProjectWorktreeEntryView[],
    error: string | null,
  ) => void = () => {}
  onProjectPathInspection: (
    path: string,
    currentBranch: string | null,
    warning: BranchWarningView | null,
    error: string | null,
  ) => void = () => {}

  constructor(url: string) {
    this.url = url
  }

  // A deliberate, user-initiated entry into the connection — boot (auth off /
  // authed) and every login() share this path. Reset the reconnect bookkeeping
  // (attempts + backoff) so a fresh connect never inherits an exhausted counter
  // from a prior session: without this, logging in after the boot loop already
  // failed would start one open() and immediately give up. Mirrors reconnect(),
  // which is the same kind of deliberate re-entry.
  connect(): void {
    this.closedByUser = false
    this.attempts = 0
    this.reconnectDelay = RECONNECT_MIN_MS
    this.open()
  }

  private open(): void {
    this.onConn("connecting")
    const ws = new WebSocket(this.url)
    ws.binaryType = "arraybuffer"
    this.ws = ws

    ws.onopen = () => {
      this.reconnectDelay = RECONNECT_MIN_MS
      this.attempts = 0
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
      case "terminal_created":
        this.onTerminalCreated(message.session_id, message.terminal_id)
        break
      case "error":
        this.onError(message.message)
        break
      case "status":
        this.onStatus(message.key, message.tone, message.message)
        break
      case "status_cleared":
        this.onStatusCleared(message.key)
        break
      case "commit_message":
        this.onCommitMessage(message.session_id, message.message)
        break
      case "commit_message_snapshot":
        this.onCommitMessageSnapshot(message.session_id, message.message)
        break
      case "dir_entries":
        this.onDirEntries(message.path, message.entries, message.error)
        break
      case "agent_name":
        this.onAgentName(message.name)
        break
      case "project_worktrees":
        this.onProjectWorktrees(
          message.project_id,
          message.entries,
          message.error,
        )
        break
      case "project_path_inspection":
        this.onProjectPathInspection(
          message.path,
          message.current_branch,
          message.warning,
          message.error,
        )
        break
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return
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

  reconnect(): void {
    this.attempts = 0
    this.closedByUser = false
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.open()
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

  subscribeTerminal(terminalId: string): void {
    this.sendJson({ type: "subscribe_terminal", terminal_id: terminalId })
  }

  createTerminal(sessionId: string): void {
    this.sendJson({ type: "create_terminal", session_id: sessionId })
  }

  resize(sessionId: string, rows: number, cols: number): void {
    this.sendJson({ type: "resize", session_id: sessionId, rows, cols })
  }

  browseDir(path: string | null): void {
    this.sendJson({ type: "browse_dir", path })
  }

  generateAgentName(): void {
    this.sendJson({ type: "generate_agent_name" })
  }

  listProjectWorktrees(projectId: string): void {
    this.sendJson({ type: "list_project_worktrees", project_id: projectId })
  }

  inspectProjectPath(path: string): void {
    this.sendJson({ type: "inspect_project_path", path })
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
