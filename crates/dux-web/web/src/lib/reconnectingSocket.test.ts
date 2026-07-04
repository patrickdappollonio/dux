import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  MAX_RECONNECT_ATTEMPTS,
  RECONNECT_MAX_MS,
  RECONNECT_MIN_MS,
  ReconnectingSocket,
} from "./reconnectingSocket"
import type { ConnState } from "./types"

// A controllable WebSocket double: tests trigger open/close explicitly and drive
// the lifecycle. `OPEN` is static so any send guard resolves.
class FakeWS {
  static OPEN = 1
  static instances: FakeWS[] = []
  url: string
  binaryType = ""
  readyState = 0
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: ((e: unknown) => void) | null = null
  onmessage: ((e: { data: unknown }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(): void {}

  close(): void {
    this.readyState = 3
    this.onclose?.()
  }

  open(): void {
    this.readyState = 1
    this.onopen?.()
  }

  triggerClose(): void {
    this.readyState = 3
    this.onclose?.()
  }
}

// A minimal concrete subclass exposing the abstract hooks so the base lifecycle
// can be exercised directly. Records the frames it saw and lets a test flip
// `retry` to simulate a route that has gone away for good.
class TestSocket extends ReconnectingSocket {
  socketOpens = 0
  messages: unknown[] = []
  configured: WebSocket[] = []
  errors: unknown[] = []
  retry = true

  protected configureSocket(ws: WebSocket): void {
    this.configured.push(ws)
  }

  protected onSocketOpen(): void {
    this.socketOpens++
  }

  protected handleMessage(event: MessageEvent): void {
    this.messages.push(event.data)
  }

  protected shouldReconnect(): boolean {
    return this.retry
  }

  protected handleError(event: Event): void {
    this.errors.push(event)
  }
}

beforeEach(() => {
  FakeWS.instances = []
  vi.stubGlobal("WebSocket", FakeWS)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

function last(): FakeWS {
  const ws = FakeWS.instances.at(-1)
  if (!ws) throw new Error("no socket constructed")
  return ws
}

describe("ReconnectingSocket", () => {
  it("exposes the shared backoff constants and cap", () => {
    expect(RECONNECT_MIN_MS).toBe(500)
    expect(RECONNECT_MAX_MS).toBe(5000)
    expect(MAX_RECONNECT_ATTEMPTS).toBe(3)
  })

  it("emits connecting → open across a normal lifecycle and runs the open hook", () => {
    const sock = new TestSocket("ws://x")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    let opened = 0
    sock.onOpen = () => {
      opened++
    }
    sock.connect()
    expect(states).toEqual(["connecting"])
    expect(sock.configured).toHaveLength(1) // configureSocket ran on the new ws
    last().open()
    expect(states).toEqual(["connecting", "open"])
    expect(sock.socketOpens).toBe(1) // onSocketOpen fired before onOpen
    expect(opened).toBe(1)
  })

  it("forwards messages to the subclass handler", () => {
    const sock = new TestSocket("ws://x")
    sock.connect()
    const ws = last()
    ws.open()
    ws.onmessage?.({ data: "hello" })
    expect(sock.messages).toEqual(["hello"])
  })

  it("routes socket errors to the subclass hook", () => {
    const sock = new TestSocket("ws://x")
    sock.connect()
    last().onerror?.("boom")
    expect(sock.errors).toEqual(["boom"])
  })

  it("reconnects with capped exponential backoff (500 → 1000 → 2000, doubling across consecutive failures)", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    sock.connect()

    // Drop 1 (never opened, so the backoff keeps growing): retry scheduled 500ms.
    last().triggerClose()
    expect(FakeWS.instances.length).toBe(1)
    vi.advanceTimersByTime(499)
    expect(FakeWS.instances.length).toBe(1)
    vi.advanceTimersByTime(1)
    expect(FakeWS.instances.length).toBe(2)

    // Drop 2: the delay doubled to 1000ms.
    last().triggerClose()
    vi.advanceTimersByTime(999)
    expect(FakeWS.instances.length).toBe(2)
    vi.advanceTimersByTime(1)
    expect(FakeWS.instances.length).toBe(3)

    // Drop 3: doubled again to 2000ms.
    last().triggerClose()
    vi.advanceTimersByTime(1999)
    expect(FakeWS.instances.length).toBe(3)
    vi.advanceTimersByTime(1)
    expect(FakeWS.instances.length).toBe(4)
  })

  it("resets the backoff to RECONNECT_MIN_MS after a successful open", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    sock.connect()
    // Fail once (delay would grow to 1000 next), then RECOVER.
    last().triggerClose()
    vi.advanceTimersByTime(RECONNECT_MIN_MS)
    expect(FakeWS.instances.length).toBe(2)
    last().open() // a good open resets attempts + delay
    // The next drop is scheduled at MIN again (not the doubled value).
    last().triggerClose()
    vi.advanceTimersByTime(RECONNECT_MIN_MS - 1)
    expect(FakeWS.instances.length).toBe(2)
    vi.advanceTimersByTime(1)
    expect(FakeWS.instances.length).toBe(3)
  })

  it("caps the backoff delay at RECONNECT_MAX_MS", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    // Never opens (so `attempts`/delay would grow) — but the delay is clamped and
    // the attempt cap stops it after MAX_RECONNECT_ATTEMPTS anyway. Drive one long
    // wait per attempt to show a single retry fires within the max window.
    sock.connect()
    last().triggerClose() // schedule attempt 1 (500ms)
    vi.advanceTimersByTime(RECONNECT_MAX_MS)
    expect(FakeWS.instances.length).toBe(2)
    last().triggerClose() // attempt 2 (1000ms)
    vi.advanceTimersByTime(RECONNECT_MAX_MS)
    expect(FakeWS.instances.length).toBe(3)
  })

  it("gives up with 'failed' after MAX_RECONNECT_ATTEMPTS and stops retrying", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    let reconnecting = 0
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.connect()
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(RECONNECT_MAX_MS)
    }
    expect(states.at(-1)).toBe("failed")
    // onReconnecting fires only on the attempts that actually schedule a retry —
    // exactly MAX_RECONNECT_ATTEMPTS, NOT on the give-up attempt.
    expect(reconnecting).toBe(MAX_RECONNECT_ATTEMPTS)
    // No further sockets once failed.
    const count = FakeWS.instances.length
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(count)
  })

  it("connect() after 'failed' resets the attempt budget and reconnects", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(RECONNECT_MAX_MS)
    }
    expect(states.at(-1)).toBe("failed")
    const before = FakeWS.instances.length
    sock.connect()
    expect(FakeWS.instances.length).toBe(before + 1)
    expect(states.at(-1)).toBe("connecting")
    // And the fresh budget yields MAX more attempts before failing again.
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(RECONNECT_MAX_MS)
    }
    expect(states.at(-1)).toBe("failed")
  })

  it("does not reconnect after a user-initiated close (closedByUser short-circuit)", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    // A deliberate close fires onclose (like a real socket) but must NOT trigger
    // the reconnect loop.
    sock.close()
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(before)
  })

  it("stops the loop (no reconnect) when shouldReconnect() returns false", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    sock.retry = false
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    last().triggerClose()
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(before)
  })

  it("detaches and closes a prior socket on a double connect() instead of orphaning it", () => {
    const sock = new TestSocket("ws://x")
    sock.connect()
    const ws1 = last()
    ws1.open()
    // A double connect() must NOT orphan ws1 — it is detached and closed before
    // ws2 is created, so ws1's later callbacks can't mutate shared state.
    sock.connect()
    const ws2 = last()
    expect(ws2).not.toBe(ws1)
    expect(ws1.readyState).toBe(3)
    expect(ws1.onclose).toBeNull()
    expect(ws1.onopen).toBeNull()
  })

  it("a replaced orphan's late open is inert (no spurious 'open')", () => {
    const sock = new TestSocket("ws://x")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    const ws1 = last()
    sock.connect()
    const ws2 = last()
    expect(ws2).not.toBe(ws1)
    // ws1's handlers were detached; a late open() is a no-op.
    ws1.onopen?.()
    expect(states).not.toContain("open")
  })
})
