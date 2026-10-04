// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// How a socket meets the auth gate: a close with 4401 or 4403 reports to the
// gate and holds the socket (no retry, no budget spent) until the page is
// signed in again, when it reattaches by itself. Fresh modules per test so the
// socket and the gate it reports to are the same instance.

class FakeWS {
  static OPEN = 1
  static instances: FakeWS[] = []
  sent: unknown[] = []
  url: string
  readyState = 0
  onopen: (() => void) | null = null
  onclose: ((e: { code: number }) => void) | null = null
  onerror: ((e: unknown) => void) | null = null
  onmessage: ((e: { data: unknown }) => void) | null = null
  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }
  send(data: unknown): void {
    this.sent.push(data)
  }
  close(code = 1000): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
  open(): void {
    this.readyState = 1
    this.onopen?.()
  }
  triggerClose(code: number): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
}

function json(status: number, body: unknown): Response {
  return new Response(status === 204 ? null : JSON.stringify(body), { status })
}

let statusBody: unknown = { password_set: true, required_here: true, signed_in: true }

async function load() {
  const gate = await import("./authGate")
  const { ReconnectingSocket } = await import("./reconnectingSocket")
  class TestSocket extends ReconnectingSocket {
    reconnecting = 0
    conns: string[] = []
    constructor() {
      super("ws://test/ws/events", { attemptBudget: () => 0 })
      this.onReconnecting = () => {
        this.reconnecting++
      }
      this.onConn = (c) => {
        this.conns.push(c)
      }
    }
    protected onSocketOpen(): void {}
    protected handleMessage(): void {}
  }
  await gate.initAuthGate()
  return { gate, socket: new TestSocket() }
}

const sockets: { dispose(): void }[] = []

beforeEach(() => {
  FakeWS.instances = []
  statusBody = { password_set: true, required_here: true, signed_in: true }
  vi.stubGlobal("WebSocket", FakeWS)
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      const url = String(input)
      if (url.endsWith("/auth/status")) return json(200, statusBody)
      if (url.endsWith("/auth/login")) return json(204, undefined)
      return json(200, {})
    }),
  )
  vi.resetModules()
})

afterEach(() => {
  for (const s of sockets.splice(0)) s.dispose()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

function last(): FakeWS {
  const ws = FakeWS.instances.at(-1)
  if (!ws) throw new Error("no socket")
  return ws
}

describe("a socket closed for auth", () => {
  it("4401 signs the page out and schedules no retry", async () => {
    vi.useFakeTimers()
    const { gate, socket } = await load()
    sockets.push(socket)
    socket.connect()
    last().open()
    statusBody = { password_set: true, required_here: true, signed_in: false }
    last().triggerClose(4401)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
    expect(socket.reconnecting).toBe(0)
    const opened = FakeWS.instances.length
    await vi.advanceTimersByTimeAsync(120_000)
    expect(FakeWS.instances.length).toBe(opened)
  })

  it("4403 shows the blocked page", async () => {
    const { gate, socket } = await load()
    sockets.push(socket)
    socket.connect()
    last().open()
    last().triggerClose(4403)
    expect(gate.getAuthPhase().kind).toBe("blocked")
  })

  it("reattaches by itself once the page is signed in again", async () => {
    const { gate, socket } = await load()
    sockets.push(socket)
    socket.connect()
    last().open()
    statusBody = { password_set: true, required_here: true, signed_in: false }
    last().triggerClose(4401)
    const before = FakeWS.instances.length
    statusBody = { password_set: true, required_here: true, signed_in: true }
    await gate.signIn("pw")
    expect(FakeWS.instances.length).toBe(before + 1)
  })

  it("does not open while the page is signed out, even when asked to", async () => {
    const { gate, socket } = await load()
    sockets.push(socket)
    statusBody = { password_set: true, required_here: true, signed_in: false }
    gate.reportUnauthorized()
    socket.connect()
    expect(FakeWS.instances.length).toBe(0)
  })

  it("an ordinary drop while signed out holds instead of retrying", async () => {
    vi.useFakeTimers()
    const { gate, socket } = await load()
    sockets.push(socket)
    socket.connect()
    last().open()
    statusBody = { password_set: true, required_here: true, signed_in: false }
    gate.reportUnauthorized()
    last().triggerClose(1006)
    const opened = FakeWS.instances.length
    await vi.advanceTimersByTimeAsync(120_000)
    expect(FakeWS.instances.length).toBe(opened)
  })

  it("a disposed socket is not revived by a sign-in", async () => {
    const { gate, socket } = await load()
    socket.connect()
    last().open()
    last().triggerClose(4401)
    socket.dispose()
    const before = FakeWS.instances.length
    await gate.signIn("pw")
    expect(FakeWS.instances.length).toBe(before)
  })
})

describe("the order a socket says things in", () => {
  it("reports an auth close to the gate before anyone hears a generic close", async () => {
    const { gate, socket } = await load()
    sockets.push(socket)
    const phaseAtClose: string[] = []
    socket.onConn = (c) => {
      if (c === "closed") phaseAtClose.push(gate.getAuthPhase().kind)
    }
    socket.connect()
    last().open()
    statusBody = { password_set: true, required_here: true, signed_in: false }
    last().triggerClose(4401)
    expect(phaseAtClose).toEqual(["signed_out"])
  })
})

describe("a PTY socket while signed out", () => {
  it("sends no keystroke, no resize and no beat", async () => {
    const gate = await import("./authGate")
    const { PtySocket } = await import("./ptySocket")
    const { noteServerValidated } = await import("./serverValidated")
    await gate.initAuthGate()
    noteServerValidated()
    const sock = new PtySocket("ws://test/ws/sessions/s1/pty")
    sockets.push(sock)
    sock.connect()
    const ws = last()
    ws.open()
    expect(sock.sendResize(24, 80)).toBe(true)
    expect(ws.sent).toHaveLength(1)

    statusBody = { password_set: true, required_here: true, signed_in: false }
    gate.reportUnauthorized()
    sock.sendInput(new TextEncoder().encode("rm -rf ~\r"))
    expect(sock.sendResize(10, 20)).toBe(false)
    expect(sock.sendBeat(1, true)).toBe(false)
    expect(ws.sent).toHaveLength(1)
  })
})

describe("a PTY socket that stayed open across a sign-out", () => {
  it("delivers the size it could not send once signed in again", async () => {
    const gate = await import("./authGate")
    const { PtySocket } = await import("./ptySocket")
    const { noteServerValidated } = await import("./serverValidated")
    await gate.initAuthGate()
    noteServerValidated()
    const sock = new PtySocket("ws://test/ws/sessions/s1/pty")
    sockets.push(sock)
    sock.connect()
    const ws = last()
    ws.open()

    statusBody = { password_set: true, required_here: true, signed_in: false }
    gate.reportUnauthorized()
    // The window changed while the login page was up.
    expect(sock.sendResize(30, 100)).toBe(false)
    expect(ws.sent).toHaveLength(0)

    statusBody = { password_set: true, required_here: true, signed_in: true }
    await gate.signIn("pw")
    expect(ws.sent.map((f) => JSON.parse(String(f)))).toEqual([{ rows: 30, cols: 100 }])
  })
})
