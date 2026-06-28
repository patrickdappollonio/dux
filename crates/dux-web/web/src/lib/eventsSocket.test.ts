import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { EventsSocket } from "./eventsSocket"
import type { ConnState, EventsServerMessage, ResourceEvent } from "./types"

// A controllable WebSocket double: tests trigger open/message/close explicitly
// and inspect the frames the socket sent. `OPEN` is static so the socket's
// `readyState === WebSocket.OPEN` send guard resolves.
class FakeWS {
  static OPEN = 1
  static instances: FakeWS[] = []
  url: string
  readyState = 0
  sent: string[] = []
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(data: string): void {
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

  message(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }

  triggerClose(): void {
    this.readyState = 3
    this.onclose?.()
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

function lastFrame(ws: FakeWS): unknown {
  const raw = ws.sent.at(-1)
  if (raw === undefined) throw new Error("nothing sent")
  return JSON.parse(raw)
}

describe("EventsSocket", () => {
  it("sends a subscribe frame for newly-added topics when open", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws = last()
    ws.open()
    sock.subscribe(["a", "b"])
    expect(lastFrame(ws)).toEqual({ subscribe: ["a", "b"] })
    expect(sock.topics).toEqual(["a", "b"])
  })

  it("does not re-send an already-subscribed topic", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws = last()
    ws.open()
    sock.subscribe(["a"])
    const count = ws.sent.length
    sock.subscribe(["a"])
    expect(ws.sent.length).toBe(count)
  })

  it("sends an unsubscribe frame and drops the topic from the set", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws = last()
    ws.open()
    sock.subscribe(["a", "b"])
    sock.unsubscribe(["a"])
    expect(lastFrame(ws)).toEqual({ unsubscribe: ["a"] })
    expect(sock.topics).toEqual(["b"])
  })

  it("buffers subscriptions made while closed and sends the whole set on open", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws = last()
    // Not open yet: subscribe only records interest, no frame on the wire.
    sock.subscribe(["x", "y"])
    expect(ws.sent).toHaveLength(0)
    ws.open()
    expect(lastFrame(ws)).toEqual({ subscribe: ["x", "y"] })
  })

  it("re-sends the entire interest set on reconnect", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    let ws = last()
    ws.open()
    sock.subscribe(["x", "y"])
    // Drop the connection; the socket schedules a reconnect via setTimeout.
    ws.triggerClose()
    vi.advanceTimersByTime(600)
    ws = last()
    expect(ws).not.toBe(FakeWS.instances[0])
    ws.open()
    expect(JSON.parse(ws.sent[0])).toEqual({ subscribe: ["x", "y"] })
  })

  it("forwards resource events to onEvent", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    const events: ResourceEvent[] = []
    sock.onEvent = (e) => events.push(e)
    sock.connect()
    const ws = last()
    ws.open()
    ws.message({ event: "session.changes", id: "s1", rev: 3 })
    expect(events).toEqual([{ event: "session.changes", id: "s1", rev: 3 }])
  })

  it("ignores malformed frames", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    const events: ResourceEvent[] = []
    sock.onEvent = (e) => events.push(e)
    sock.connect()
    const ws = last()
    ws.open()
    ws.onmessage?.({ data: "not json" })
    ws.message({ noEvent: true })
    expect(events).toHaveLength(0)
  })

  it("does not reconnect after a user-initiated close", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws = last()
    ws.open()
    const before = FakeWS.instances.length
    // A deliberate close fires onclose (like a real socket) but must NOT trigger
    // the reconnect loop — closedByUser short-circuits it.
    sock.close()
    vi.advanceTimersByTime(10000)
    expect(FakeWS.instances.length).toBe(before)
  })

  it("fires onOpen after (re)sending the set", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    let opened = 0
    sock.onOpen = () => {
      opened++
    }
    sock.connect()
    last().open()
    expect(opened).toBe(1)
  })

  it("emits connecting → open across a normal lifecycle", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    expect(states).toEqual(["connecting"])
    last().open()
    expect(states).toEqual(["connecting", "open"])
  })

  it("re-emits 'connecting' on each reconnect attempt and 'closed' on every drop", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    last().open()
    last().triggerClose()
    // closed now; the backoff timer fires a fresh open() → connecting.
    vi.advanceTimersByTime(600)
    expect(states).toEqual(["connecting", "open", "closed", "connecting"])
  })

  it("gives up with 'failed' after exhausting reconnect attempts", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    // The socket never opens (the server keeps rejecting): each close schedules a
    // capped-backoff reconnect until the attempt budget is spent, then "failed".
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

  it("a manual connect() after 'failed' resets the budget and retries", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(5000)
    }
    expect(states.at(-1)).toBe("failed")
    // The Reconnect button calls connect() again; it opens a fresh socket.
    const before = FakeWS.instances.length
    sock.connect()
    expect(FakeWS.instances.length).toBe(before + 1)
    expect(states.at(-1)).toBe("connecting")
  })

  it("open() closes and detaches a prior socket instead of orphaning it", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws1 = last()
    ws1.open()
    sock.subscribe(["a"])
    // A double connect() (double-click Reconnect, or a recheck firing connect()
    // mid-reconnect) must NOT orphan ws1 — it is closed before ws2 is created.
    sock.connect()
    const ws2 = last()
    expect(ws2).not.toBe(ws1)
    expect(ws1.readyState).toBe(3) // the prior socket was closed
    ws2.open()
    // The whole interest set re-rides the fresh socket.
    expect(JSON.parse(ws2.sent[0])).toEqual({ subscribe: ["a"] })
  })

  it("a replaced orphan's late close cannot null the live socket", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws1 = last()
    ws1.open()
    sock.subscribe(["a"])
    sock.connect()
    const ws2 = last()
    ws2.open()
    // Simulate the orphan firing a late close AFTER ws2 became live. The bug:
    // its onclose would `this.ws = null`, killing all outbound frames. With the
    // detach + identity guard it is inert, so the live socket keeps sending.
    ws1.triggerClose()
    sock.subscribe(["b"])
    expect(JSON.parse(ws2.sent.at(-1) as string)).toEqual({ subscribe: ["b"] })
  })

  it("a replaced orphan's late open is inert (no spurious 'open')", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect() // ws1 still connecting
    const ws1 = last()
    sock.connect() // replace before ws1 opened → ws2 connecting
    const ws2 = last()
    expect(ws2).not.toBe(ws1)
    // A late open() from the orphan must not emit "open" or re-send interest.
    ws1.open()
    expect(states).not.toContain("open")
  })

  it("forwards control frames (connected/status) to onEvent", () => {
    const sock = new EventsSocket("ws://x/ws/events")
    const events: EventsServerMessage[] = []
    sock.onEvent = (e) => events.push(e)
    sock.connect()
    const ws = last()
    ws.open()
    ws.message({ event: "connected", id: "conn-1" })
    ws.message({ event: "status", key: "k", tone: "info", message: "hi", scope: "all" })
    expect(events).toEqual([
      { event: "connected", id: "conn-1" },
      { event: "status", key: "k", tone: "info", message: "hi", scope: "all" },
    ])
  })
})
