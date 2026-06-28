import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  terminalPtyUrl,
} from "./ptySocket"

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
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((e: { data: string | ArrayBuffer }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(data: string | ArrayBuffer): void {
    this.sent.push(data)
  }

  close(): void {
    this.readyState = 3
    this.onclose?.()
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

  triggerClose(): void {
    this.readyState = 3
    this.onclose?.()
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

  it("encodes ids and uses wss under https", () => {
    vi.stubGlobal("location", { protocol: "https:", host: "example.com" })
    expect(agentPtyUrl("a b")).toBe("wss://example.com/ws/sessions/a%20b/pty")
    expect(terminalPtyUrl("s/1", "t/2")).toBe(
      "wss://example.com/ws/sessions/s%2F1/terminals/t%2F2/pty",
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

  it("keeps retrying with no hard attempt cap", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    // Six failed opens in a row: well past the legacy DuxSocket's 4-attempt cap.
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(5000)
    }
    // A fresh socket was constructed for each retry — it never gave up.
    expect(FakeWS.instances.length).toBeGreaterThan(6)
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
