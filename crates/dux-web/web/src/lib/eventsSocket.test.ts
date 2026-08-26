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
  onclose: ((e: { code: number }) => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(data: string): void {
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

  message(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }

  triggerClose(code = 1006): void {
    this.readyState = 3
    this.onclose?.({ code })
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

  it("chunks the resend into per-frame-capped subscribe frames on reconnect", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    let ws = last()
    ws.open()
    // More than the per-frame cap (64): 130 topics. Subscribing while open sends a
    // single (uncapped) delta frame; the CHUNKING is applied by the base's
    // `onSocketOpen` resend, which we exercise via a reconnect below.
    const topics = Array.from({ length: 130 }, (_, i) => `t${i}`)
    sock.subscribe(topics)
    // Drop the connection; the base schedules a reconnect via setTimeout.
    ws.triggerClose()
    vi.advanceTimersByTime(600)
    ws = last()
    expect(ws).not.toBe(FakeWS.instances[0])
    // On reopen the WHOLE interest set is re-sent, split into frames of at most
    // MAX_EVENT_TOPICS_PER_FRAME (64): 64 + 64 + 2.
    ws.open()
    const frames = ws.sent.map(
      (raw) => JSON.parse(raw) as { subscribe: string[] },
    )
    expect(frames.map((f) => f.subscribe.length)).toEqual([64, 64, 2])
    // No frame exceeds the cap, and the frames together cover the whole set in
    // order (nothing dropped off the tail).
    expect(frames.every((f) => f.subscribe.length <= 64)).toBe(true)
    expect(frames.flatMap((f) => f.subscribe)).toEqual(topics)
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

  it("leaves a breadcrumb when a frame cannot be parsed", () => {
    // A dropped frame used to be entirely silent, and the frames on this socket
    // now include the whole workspace document: a truncated or malformed one
    // would show up as a sidebar that quietly stops updating, with nothing
    // anywhere to say why. The size is in the message because size is the most
    // likely thing to have gone wrong.
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const sock = new EventsSocket("ws://x/ws/events")
    sock.connect()
    const ws = last()
    ws.open()
    ws.onmessage?.({ data: '{"event":"workspace","workspace":{' })
    expect(warn).toHaveBeenCalledTimes(1)
    expect(String(warn.mock.calls[0]?.[0])).toContain("34")
    warn.mockRestore()
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

  // FLIPPED, both of these. They used to assert the events socket gave up after
  // its attempt budget and emitted `failed`, and that a manual connect() then
  // restored it. The budget is gone: the spine is what the whole app rides on,
  // and abandoning it while the tab is open only ever produced a page that had
  // quietly stopped updating. It retries indefinitely instead.
  //
  // The events socket does NOT park while hidden, which is the other half of the
  // policy: attention indicators and OS notifications ride this socket precisely
  // when the tab is in the background.
  it("never gives up: it retries indefinitely, hidden or not", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    for (let i = 0; i < 20; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(60000)
    }
    expect(states).not.toContain("failed")
    expect(FakeWS.instances.length).toBe(21)
    sock.close()
  })

  it("a manual connect() attempts at once instead of waiting out the backoff", () => {
    vi.useFakeTimers()
    const sock = new EventsSocket("ws://x/ws/events")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    for (let i = 0; i < 6; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(60000)
    }
    // The Reconnect button calls connect() again; it opens a fresh socket
    // immediately rather than sitting out the grown gap.
    const before = FakeWS.instances.length
    sock.connect()
    expect(FakeWS.instances.length).toBe(before + 1)
    expect(states.at(-1)).toBe("connecting")
    sock.close()
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
