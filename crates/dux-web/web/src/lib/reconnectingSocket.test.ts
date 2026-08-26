// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS,
  publishConnectionTiming,
} from "./connectionTiming"
import {
  CONNECT_TIMEOUT_MS,
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
  onclose: ((e: { code: number }) => void) | null = null
  onerror: ((e: unknown) => void) | null = null
  onmessage: ((e: { data: unknown }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(): void {}

  close(code = 1000): void {
    this.readyState = 3
    this.onclose?.({ code })
  }

  open(): void {
    this.readyState = 1
    this.onopen?.()
  }

  triggerClose(code = 1006): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
}

// A minimal concrete subclass exposing the abstract hooks so the base lifecycle
// can be exercised directly. Records the frames it saw and lets a test flip
// `retry` to simulate a route that has gone away for good.
class TestSocket extends ReconnectingSocket {
  // Every socket built by a test, so `afterEach` can close them. A live socket
  // keeps its four wake listeners on the shared `window`/`document`, so one left
  // open by a finished test would answer the next test's wake signal and open a
  // socket of its own.
  static built: TestSocket[] = []
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

  protected shouldReconnect(closeCode: number): boolean {
    void closeCode
    return this.retry
  }

  protected handleError(event: Event): void {
    this.errors.push(event)
  }

  constructor(url: string, policy: ConstructorParameters<typeof ReconnectingSocket>[1] = {}) {
    super(url, policy)
    TestSocket.built.push(this)
  }
}

beforeEach(() => {
  FakeWS.instances = []
  vi.stubGlobal("WebSocket", FakeWS)
})

afterEach(() => {
  for (const sock of TestSocket.built.splice(0)) sock.dispose()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

function last(): FakeWS {
  const ws = FakeWS.instances.at(-1)
  if (!ws) throw new Error("no socket constructed")
  return ws
}

describe("ReconnectingSocket", () => {
  it("exposes the shared backoff floor, and takes its ceiling from config", () => {
    expect(RECONNECT_MIN_MS).toBe(500)
    expect(DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS).toBe(10)
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

  it("caps the backoff delay at the CONFIGURED ceiling, not a compiled-in one", () => {
    vi.useFakeTimers()
    publishConnectionTiming({ reconnect_backoff_cap_seconds: 2 })
    const sock = new TestSocket("ws://x")
    sock.connect()
    // 500, 1000, then clamped at 2000 forever.
    const delays = [500, 1000, 2000, 2000]
    let expected = 1
    for (const delay of delays) {
      last().triggerClose()
      vi.advanceTimersByTime(delay - 1)
      expect(FakeWS.instances.length).toBe(expected)
      vi.advanceTimersByTime(1)
      expected++
      expect(FakeWS.instances.length).toBe(expected)
    }
  })

  // FLIPPED. This used to assert the socket gave up after MAX_RECONNECT_ATTEMPTS
  // and emitted `failed`. The budget is gone: a phone on a train is not a server
  // that is down, and the measured truth was that the budget was never spent
  // anyway (every successful open refilled it, so 21 consecutive open-then-close
  // cycles never produced the give-up state). `failed` is now reserved for a
  // terminal close code, which `shouldReconnect` owns.
  it("never gives up on transient closes: it retries indefinitely while visible", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    let reconnecting = 0
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.connect()
    // Each cycle is a real one: the socket opens, drops, and the retry brings a
    // fresh one. The advance stays under `CONNECT_TIMEOUT_MS` so the count is
    // purely drops, with no socket abandoned for never opening.
    for (let i = 0; i < 20; i++) {
      last().open()
      last().triggerClose()
      vi.advanceTimersByTime(10_000)
    }
    expect(states).not.toContain("failed")
    // One cue per drop, and one fresh socket per drop.
    expect(reconnecting).toBe(20)
    expect(FakeWS.instances.length).toBe(21)
  })

  it("connect() resets the backoff to the minimum", () => {
    vi.useFakeTimers()
    const sock = new TestSocket("ws://x")
    sock.connect()
    // Grow the delay well past the floor.
    for (let i = 0; i < 5; i++) {
      last().triggerClose()
      vi.advanceTimersByTime(60000)
    }
    // A manual Reconnect: the next drop waits the FLOOR again, not the grown
    // delay.
    sock.connect()
    const before = FakeWS.instances.length
    last().triggerClose()
    vi.advanceTimersByTime(RECONNECT_MIN_MS - 1)
    expect(FakeWS.instances.length).toBe(before)
    vi.advanceTimersByTime(1)
    expect(FakeWS.instances.length).toBe(before + 1)
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

/// `document.visibilityState`, which decides whether a parking socket may
/// schedule anything at all.
function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  })
}

describe("parking while hidden (a PTY-only policy)", () => {
  it("schedules nothing and burns no timer while the page is hidden", () => {
    vi.useFakeTimers()
    setVisibility("hidden")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    last().triggerClose()
    vi.advanceTimersByTime(600000)
    // Still the one socket the connect() made: a hidden page retries nothing.
    expect(FakeWS.instances.length).toBe(1)
    expect(vi.getTimerCount()).toBe(0)
    setVisibility("visible")
  })

  it("keeps retrying while hidden when the policy is off, because attention rides that socket", () => {
    vi.useFakeTimers()
    setVisibility("hidden")
    const sock = new TestSocket("ws://x")
    sock.connect()
    last().open()
    last().triggerClose()
    vi.advanceTimersByTime(10_000)
    expect(FakeWS.instances.length).toBe(2)
    setVisibility("visible")
  })
})

describe("the four wake signals", () => {
  function parked(): TestSocket {
    setVisibility("hidden")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    last().triggerClose()
    vi.advanceTimersByTime(600000)
    expect(FakeWS.instances.length).toBe(1)
    return sock
  }

  it.each([
    [
      "visibilitychange",
      () => document.dispatchEvent(new Event("visibilitychange")),
    ],
    ["pageshow", () => window.dispatchEvent(new Event("pageshow"))],
    ["focus", () => window.dispatchEvent(new Event("focus"))],
    ["online", () => window.dispatchEvent(new Event("online"))],
  ])("%s unparks and attempts immediately", (_name, fire) => {
    vi.useFakeTimers()
    parked()
    setVisibility("visible")
    fire()
    expect(FakeWS.instances.length).toBe(2)
  })

  it("all four in the same tick produce EXACTLY ONE attempt", () => {
    vi.useFakeTimers()
    parked()
    setVisibility("visible")
    document.dispatchEvent(new Event("visibilitychange"))
    window.dispatchEvent(new Event("pageshow"))
    window.dispatchEvent(new Event("focus"))
    window.dispatchEvent(new Event("online"))
    expect(FakeWS.instances.length).toBe(2)
  })

  it("never tears down a LIVE socket, which is what a returning phone would ask for four times over", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    const live = last()
    live.open()
    window.dispatchEvent(new Event("focus"))
    window.dispatchEvent(new Event("online"))
    window.dispatchEvent(new Event("pageshow"))
    document.dispatchEvent(new Event("visibilitychange"))
    expect(FakeWS.instances.length).toBe(1)
    expect(live.readyState).toBe(1)
  })

  it("is a no-op on a socket that is still CONNECTING", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    window.dispatchEvent(new Event("focus"))
    expect(FakeWS.instances.length).toBe(1)
  })

  // A LIFECYCLE CLOSE KEEPS ITS LISTENERS, and this test used to say the
  // opposite. `pagehide` routes to `close()`, and a return is not always a
  // `pageshow`: a phone unlocking, a tab being switched back to, or a network
  // coming back can announce themselves through visibility, focus or online
  // alone. With the listeners detached by the close, those returns found nobody
  // home and both sockets stayed dead until the user pressed Reconnect.
  it("reopens on a wake signal after a lifecycle close", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    last().open()
    sock.close()
    const before = FakeWS.instances.length
    window.dispatchEvent(new Event("focus"))
    expect(FakeWS.instances.length).toBe(before + 1)
  })

  it("reopens on a bare visibilitychange after a pagehide-shaped close", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    last().open()
    sock.close()
    const before = FakeWS.instances.length
    document.dispatchEvent(new Event("visibilitychange"))
    expect(FakeWS.instances.length).toBe(before + 1)
  })

  // DISPOSE IS THE REAL TEARDOWN, and the only thing that detaches them. A pane
  // that unmounted must never be revived by a window event.
  it("is a no-op after dispose(), whose listeners really are gone", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.connect()
    last().open()
    sock.dispose()
    const before = FakeWS.instances.length
    window.dispatchEvent(new Event("focus"))
    document.dispatchEvent(new Event("visibilitychange"))
    window.dispatchEvent(new Event("pageshow"))
    window.dispatchEvent(new Event("online"))
    expect(FakeWS.instances.length).toBe(before)
  })

  // A DISPOSED SOCKET IS DEAD FOR GOOD. `close()` and a terminal close code are
  // both recoverable by a deliberate `connect()` (the Reconnect button), and
  // that is why `connect()` clears the stop flag. Disposal is not: the pane that
  // owned this socket is gone, so reviving it would open a connection nothing is
  // listening to and, for a PTY, launch a provider for a pane that unmounted.
  it("refuses to reconnect once it has been disposed", () => {
    const sock = new TestSocket("ws://x")
    sock.connect()
    last().open()
    sock.dispose()
    const before = FakeWS.instances.length
    sock.connect()
    expect(FakeWS.instances.length).toBe(before)
  })

  it("is a no-op once the route is gone for good, where shouldReconnect said no", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x", { parkWhileHidden: true })
    sock.retry = false
    sock.connect()
    last().open()
    last().triggerClose()
    const before = FakeWS.instances.length
    window.dispatchEvent(new Event("focus"))
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(before)
  })
})

// A SOCKET WEDGED IN CONNECTING IS THE ONE STATE NOTHING ELSE RESCUES.
// `resumeNow` returns early while `this.ws` is non-null, deliberately, so a wake
// signal cannot tear down a connection that is working. That makes all four wake
// signals inert against a socket that is still connecting, and no retry timer is
// armed either. Even the ordinary case is bad enough: an operating system's own
// connect timeout can be a minute or two, and for all of it a returning phone
// taps a button that does nothing.
// The heartbeat's missed answer is the only caller, and it can fire against a
// socket that is not open: during an outage the deadline keeps running while the
// retry path is mid-attempt, and dropping the CONNECTING socket restarts the
// reconnect from the beginning, once per deadline, for as long as the outage
// lasts.
describe("dropForRetry", () => {
  it("drops a socket that is OPEN, which is the case it exists for", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x")
    sock.connect()
    const live = last()
    live.open()
    sock.dropForRetry()
    expect(live.readyState).toBe(3)
  })

  it("leaves a CONNECTING attempt alone", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x")
    sock.connect()
    const attempt = last()
    expect(attempt.readyState).toBe(0)
    sock.dropForRetry()
    expect(attempt.readyState).toBe(0)
    expect(FakeWS.instances.length).toBe(1)
  })
})

describe("the connect deadline", () => {
  it("abandons a socket that never opens and retries", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    expect(FakeWS.instances).toHaveLength(1)
    // Just short of the deadline: still waiting, still hopeful.
    vi.advanceTimersByTime(CONNECT_TIMEOUT_MS - 1)
    expect(FakeWS.instances).toHaveLength(1)
    vi.advanceTimersByTime(1 + RECONNECT_MIN_MS)
    expect(FakeWS.instances).toHaveLength(2)
    expect(states).toContain("closed")
  })

  it("does not fire against a socket that opened in time", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x")
    sock.connect()
    last().open()
    vi.advanceTimersByTime(CONNECT_TIMEOUT_MS * 4)
    expect(FakeWS.instances).toHaveLength(1)
  })

  it("does not fire against a socket the app deliberately closed", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    const sock = new TestSocket("ws://x")
    sock.connect()
    sock.close()
    vi.advanceTimersByTime(CONNECT_TIMEOUT_MS * 4)
    expect(FakeWS.instances).toHaveLength(1)
  })
})

describe("the canRetry gate", () => {
  it("holds every retry while the gate is shut, and releases the moment it opens", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    let allowed = true
    const sock = new TestSocket("ws://x", { canRetry: () => allowed })
    sock.connect()
    last().open()
    allowed = false
    last().triggerClose()
    // The gate is shut: the timer keeps re-arming rather than opening a socket,
    // so nothing force-launches a provider on a server we have not identified.
    vi.advanceTimersByTime(600000)
    expect(FakeWS.instances.length).toBe(1)
    allowed = true
    vi.advanceTimersByTime(10_000)
    expect(FakeWS.instances.length).toBe(2)
  })

  it("holds a wake signal too, rather than letting a return bypass it", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    let allowed = true
    const sock = new TestSocket("ws://x", { canRetry: () => allowed })
    sock.connect()
    last().open()
    allowed = false
    last().triggerClose()
    window.dispatchEvent(new Event("focus"))
    expect(FakeWS.instances.length).toBe(1)
  })

  // THE GATE GUARDS THE GESTURES TOO. `connect()` is the mount attach, the
  // take-over bounce, the heal bounce and the Reconnect button, and it used to
  // walk straight past the policy: only automatic retries were ever gated. A
  // user tapping an agent while the identity probe was in flight against a
  // restarted server attached and force-launched its provider on the new run.
  it("DEFERS connect() itself while the gate is shut, and opens once it clears", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    let allowed = false
    const sock = new TestSocket("ws://x", { canRetry: () => allowed })
    sock.connect()
    expect(FakeWS.instances).toHaveLength(0)
    allowed = true
    vi.advanceTimersByTime(RECONNECT_MIN_MS)
    expect(FakeWS.instances).toHaveLength(1)
    // And it was a genuine reset: the deferred attempt went out at the floor,
    // not at whatever gap a previous session had grown to.
    expect(sock.socketOpens).toBe(0)
  })

  // A RETURN COSTS NOTHING WHILE THE GATE IS SHUT. A phone coming back fires
  // three or four wake signals in the same tick and the events drop that
  // preceded it is exactly what shut the gate, so a wake that cleared the armed
  // timer and armed a new one spent a doubling per signal: measured, four
  // signals turned a 500ms reattach into eight seconds.
  it("does not spend a doubling of the backoff per wake signal while the gate is shut", () => {
    vi.useFakeTimers()
    setVisibility("visible")
    let allowed = true
    const sock = new TestSocket("ws://x", { canRetry: () => allowed })
    sock.connect()
    last().open()
    allowed = false
    last().triggerClose()
    for (let i = 0; i < 4; i++) window.dispatchEvent(new Event("focus"))
    allowed = true
    vi.advanceTimersByTime(RECONNECT_MIN_MS)
    expect(FakeWS.instances).toHaveLength(2)
  })
})
