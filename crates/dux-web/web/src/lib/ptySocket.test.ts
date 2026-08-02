import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  PROVIDER_UNAVAILABLE_CLOSE,
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  projectTerminalPtyUrl,
  standaloneTerminalPtyUrl,
  tabPtyUrl,
  terminalPtyUrl,
  terminalSocketUrl,
} from "./ptySocket"
import type { ConnState } from "./types"

// A controllable WebSocket double: tests trigger open/message/close explicitly
// and inspect the frames the socket sent. `OPEN` is static so the socket's
// `readyState === WebSocket.OPEN` send guard resolves. `binaryType` is recorded
// so we can assert the socket asks for arraybuffer framing.
class FakeWS {
  static OPEN = 1
  static instances: FakeWS[] = []
  url: string
  binaryType = ""
  readyState = 0
  // Sent frames: strings (resize) and ArrayBuffers (stdin), in order.
  sent: (string | ArrayBuffer)[] = []
  onopen: (() => void) | null = null
  onclose: ((e: { code: number }) => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((e: { data: string | ArrayBuffer }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(data: string | ArrayBuffer): void {
    this.sent.push(data)
  }

  close(code = 1000): void {
    this.readyState = 3
    this.onclose?.({ code })
  }

  // Drive the lifecycle from the test.
  open(): void {
    this.readyState = 1
    this.onopen?.()
  }

  // Deliver a server→client Binary frame (raw PTY bytes).
  binary(bytes: number[]): void {
    this.onmessage?.({ data: new Uint8Array(bytes).buffer })
  }

  // Deliver a server→client Text frame (the `connected` handshake).
  text(payload: string): void {
    this.onmessage?.({ data: payload })
  }

  // Close with a code; default 1006 (abnormal) models a transient transport drop.
  triggerClose(code = 1006): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
}

beforeEach(() => {
  FakeWS.instances = []
  vi.stubGlobal("WebSocket", FakeWS)
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7070" })
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
  setActivePtySocket(null)
})

function last(): FakeWS {
  const ws = FakeWS.instances.at(-1)
  if (!ws) throw new Error("no socket constructed")
  return ws
}

describe("ptySocket URL builders", () => {
  it("builds the agent PTY URL (ws under http)", () => {
    expect(agentPtyUrl("s1")).toBe("ws://localhost:7070/ws/sessions/s1/pty")
  })

  it("builds the companion-terminal PTY URL nested under its session", () => {
    expect(terminalPtyUrl("s1", "t9")).toBe(
      "ws://localhost:7070/ws/sessions/s1/terminals/t9/pty",
    )
  })

  it("builds the project-terminal PTY URL nested under its project", () => {
    // A typo'd path here would 404 forever through the reconnecting socket
    // with no visible error, so pin the exact string.
    expect(projectTerminalPtyUrl("p1", "t9")).toBe(
      "ws://localhost:7070/ws/projects/p1/terminals/t9/pty",
    )
  })

  it("builds the standalone-terminal PTY URL un-nested, naming no owner", () => {
    expect(standaloneTerminalPtyUrl("t9")).toBe(
      "ws://localhost:7070/ws/terminals/t9/pty",
    )
  })

  it("routes each owner kind to its own socket", () => {
    // Which address a terminal is reachable at is an ownership decision, so pin
    // all three: a terminal sent to the wrong route 404s forever behind the
    // reconnecting socket, with nothing visible saying why.
    expect(terminalSocketUrl({ kind: "session", sessionId: "s1" }, "t9")).toBe(
      "ws://localhost:7070/ws/sessions/s1/terminals/t9/pty",
    )
    expect(terminalSocketUrl({ kind: "project", projectId: "p1" }, "t9")).toBe(
      "ws://localhost:7070/ws/projects/p1/terminals/t9/pty",
    )
    expect(terminalSocketUrl({ kind: "standalone" }, "t9")).toBe(
      "ws://localhost:7070/ws/terminals/t9/pty",
    )
  })

  it("builds the extra-tab PTY URL nested under its session", () => {
    expect(tabPtyUrl("s1", "tab9")).toBe(
      "ws://localhost:7070/ws/sessions/s1/tabs/tab9/pty",
    )
  })

  it("encodes ids and uses wss under https", () => {
    vi.stubGlobal("location", { protocol: "https:", host: "example.com" })
    expect(agentPtyUrl("a b")).toBe("wss://example.com/ws/sessions/a%20b/pty")
    expect(terminalPtyUrl("s/1", "t/2")).toBe(
      "wss://example.com/ws/sessions/s%2F1/terminals/t%2F2/pty",
    )
    expect(tabPtyUrl("s/1", "b/2")).toBe(
      "wss://example.com/ws/sessions/s%2F1/tabs/b%2F2/pty",
    )
    expect(projectTerminalPtyUrl("p/1", "t/2")).toBe(
      "wss://example.com/ws/projects/p%2F1/terminals/t%2F2/pty",
    )
    expect(standaloneTerminalPtyUrl("t/2")).toBe(
      "wss://example.com/ws/terminals/t%2F2/pty",
    )
  })
})

describe("PtySocket", () => {
  it("connects to the given URL and requests arraybuffer framing", () => {
    const sock = new PtySocket("ws://x/ws/sessions/s1/pty")
    sock.connect()
    const ws = last()
    expect(ws.url).toBe("ws://x/ws/sessions/s1/pty")
    expect(ws.binaryType).toBe("arraybuffer")
  })

  it("records connection id and replay generation from the connected frame", () => {
    const sock = new PtySocket("ws://x/pty")
    let connectedId: string | null = null
    sock.onConnected = (id) => {
      connectedId = id
    }
    sock.connect()
    last().open()
    expect(sock.connectionId).toBeNull()
    expect(sock.replayGeneration).toBeNull()
    last().text(JSON.stringify({ event: "connected", id: "c-1", gen: 42 }))
    expect(connectedId).toBe("c-1")
    expect(sock.connectionId).toBe("c-1")
    expect(sock.replayGeneration).toBe(42)
  })

  it("leaves replayGeneration null when the connected frame omits gen", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().text(JSON.stringify({ event: "connected", id: "c-2" }))
    expect(sock.connectionId).toBe("c-2")
    expect(sock.replayGeneration).toBeNull()
  })

  it("fires onOpen on each (re)open", () => {
    const sock = new PtySocket("ws://x/pty")
    let opens = 0
    sock.onOpen = () => {
      opens++
    }
    sock.connect()
    last().open()
    expect(opens).toBe(1)
  })

  it("streams server Binary frames to onBytes as Uint8Array", () => {
    const sock = new PtySocket("ws://x/pty")
    const chunks: Uint8Array[] = []
    sock.onBytes((b) => chunks.push(b))
    sock.connect()
    const ws = last()
    ws.open()
    // The server replays scrollback as the first Binary frame, then live bytes.
    ws.binary([0x68, 0x69]) // "hi" — the repaint
    ws.binary([0x21]) // "!" — a live byte
    expect(chunks.map((c) => Array.from(c))).toEqual([[0x68, 0x69], [0x21]])
  })

  it("sends stdin as a Binary (ArrayBuffer) frame when open", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    const ws = last()
    ws.open()
    sock.sendInput(new Uint8Array([1, 2, 3]))
    expect(ws.sent).toHaveLength(1)
    const frame = ws.sent[0]
    expect(frame).toBeInstanceOf(ArrayBuffer)
    expect(Array.from(new Uint8Array(frame as ArrayBuffer))).toEqual([1, 2, 3])
  })

  it("does not send stdin before the socket is open", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    // Not opened yet (readyState 0): the send guard drops it.
    sock.sendInput(new Uint8Array([9]))
    expect(last().sent).toHaveLength(0)
  })

  it("isOpen reflects the socket lifecycle (pre-open, open, closed)", () => {
    // The compose bar's Send checks this before writing so a message typed
    // while disconnected is kept (with a toast) instead of silently dropped by
    // the sendInput readyState guard.
    const sock = new PtySocket("ws://x/pty")
    expect(sock.isOpen).toBe(false)
    sock.connect()
    expect(sock.isOpen).toBe(false)
    const ws = last()
    ws.open()
    expect(sock.isOpen).toBe(true)
    sock.close()
    expect(sock.isOpen).toBe(false)
  })

  it("sends a resize as a Text JSON frame {rows, cols}", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    const ws = last()
    ws.open()
    sock.sendResize(40, 120)
    expect(ws.sent).toHaveLength(1)
    expect(JSON.parse(ws.sent[0] as string)).toEqual({ rows: 40, cols: 120 })
  })

  it("reconnects after an unexpected close and receives the replay (resends nothing)", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const chunks: Uint8Array[] = []
    sock.onBytes((b) => chunks.push(b))
    let opens = 0
    sock.onOpen = () => {
      opens++
    }
    sock.connect()
    let ws = last()
    ws.open()
    expect(opens).toBe(1)
    // Drop the connection; the socket schedules a reconnect via setTimeout.
    ws.triggerClose()
    vi.advanceTimersByTime(600)
    ws = last()
    expect(ws).not.toBe(FakeWS.instances[0])
    ws.open()
    expect(opens).toBe(2)
    // The reconnect sends NOTHING on its own (no buffered subscribe) — the
    // server replays scrollback as the first Binary frame after the reopen.
    expect(ws.sent).toHaveLength(0)
    ws.binary([0x41]) // the post-reconnect repaint
    expect(chunks.map((c) => Array.from(c))).toEqual([[0x41]])
  })

  it("fires onReconnecting once when the socket drops, then onOpen on recovery", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let reconnecting = 0
    let opens = 0
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.onOpen = () => {
      opens++
    }
    sock.connect()
    last().open()
    expect(opens).toBe(1)
    expect(reconnecting).toBe(0)
    // Drop: a reconnect is scheduled, so onReconnecting fires exactly once.
    last().triggerClose()
    expect(reconnecting).toBe(1)
    // The pending reconnect timer must not re-fire onReconnecting before it opens.
    vi.advanceTimersByTime(600)
    expect(reconnecting).toBe(1)
    // Recovery re-opens the socket: onOpen signals the socket is live again.
    last().open()
    expect(opens).toBe(2)
  })

  it("does not fire onReconnecting on a user-initiated close", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let reconnecting = 0
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.connect()
    last().open()
    sock.close()
    vi.advanceTimersByTime(10000)
    expect(reconnecting).toBe(0)
  })

  it("gives up with 'failed' after exhausting reconnect attempts (matches events)", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    // The socket never opens (the server keeps rejecting): each close schedules a
    // capped-backoff reconnect until the shared 3-attempt budget is spent, then
    // "failed" — the same cap the events socket uses (no more infinite retry).
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(5000)
    }
    expect(states.at(-1)).toBe("failed")
    // Once failed, the loop stops constructing sockets.
    const count = FakeWS.instances.length
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(count)
  })

  it("stops without retrying when the server closes with the provider-unavailable code", () => {
    // The provider failed to launch (missing CLI) or crashed/exited: the server
    // closes with PROVIDER_UNAVAILABLE_CLOSE, meaning "do not retry". Re-subscribing
    // would relaunch the doomed provider, so the socket must stop on the FIRST
    // such close (not loop) and surface the give-up state for the Reconnect
    // affordance — no attempt cap needed.
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    last().triggerClose(PROVIDER_UNAVAILABLE_CLOSE)
    // No reconnect scheduled, ever, and the pane's give-up state is signalled.
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(before)
    expect(states.at(-1)).toBe("failed")
  })

  it("still retries after an ordinary transient close (not the provider-unavailable code)", () => {
    // A plain transport drop (code 1006) is transient — the provider may still be
    // alive server-side — so the socket reconnects to re-attach, exactly like the
    // events socket.
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().triggerClose(1006)
    vi.advanceTimersByTime(600)
    expect(FakeWS.instances.length).toBe(2)
  })

  it("a manual connect() after 'failed' resets the budget and retries", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(5000)
    }
    expect(states.at(-1)).toBe("failed")
    // The pane's Reconnect affordance calls connect() again; a fresh socket opens.
    const before = FakeWS.instances.length
    sock.connect()
    expect(FakeWS.instances.length).toBe(before + 1)
    expect(states.at(-1)).toBe("connecting")
  })

  it("does not reconnect and fires onGone once shouldRetry says the route is gone", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let gone = 0
    let reconnecting = 0
    sock.onGone = () => {
      gone++
    }
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.shouldRetry = () => false
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    last().triggerClose()
    // No reconnect scheduled (no new socket even after time passes) and no
    // "Reconnecting…" signal — this is a hard stop, not a retry.
    vi.advanceTimersByTime(10000)
    expect(FakeWS.instances.length).toBe(before)
    expect(reconnecting).toBe(0)
    expect(gone).toBe(1)
  })

  it("still reconnects normally when shouldRetry returns true (the default)", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let gone = 0
    sock.onGone = () => {
      gone++
    }
    sock.connect()
    last().open()
    last().triggerClose()
    vi.advanceTimersByTime(600)
    expect(FakeWS.instances.length).toBe(2)
    expect(gone).toBe(0)
  })

  it("does not reconnect after a user-initiated close", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    sock.close()
    vi.advanceTimersByTime(10000)
    expect(FakeWS.instances.length).toBe(before)
  })
})

describe("active PTY socket registry", () => {
  it("stores and clears the active socket", () => {
    expect(getActivePtySocket()).toBeNull()
    const sock = new PtySocket("ws://x/pty")
    setActivePtySocket(sock)
    expect(getActivePtySocket()).toBe(sock)
    setActivePtySocket(null)
    expect(getActivePtySocket()).toBeNull()
  })
})
