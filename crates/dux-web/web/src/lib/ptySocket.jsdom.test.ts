// @vitest-environment jsdom
//
// THE BROWSER HALF of a PTY socket's return path. The pure tests beside this
// file run in node, where there is no `window` and no `document`, so
// `attachWakeSignals` quietly does nothing there and every one of the four wake
// signals is unobservable: a socket could stop answering them entirely and that
// suite would stay green. These drive the real events.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { PtySocket } from "./ptySocket"
import { clearServerValidated, noteServerValidated } from "./serverValidated"

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
}

const built: PtySocket[] = []

function socket(): PtySocket {
  const sock = new PtySocket("ws://localhost:7070/ws/sessions/s1/pty")
  built.push(sock)
  return sock
}

function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", {
    value: state,
    configurable: true,
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeWS.instances = []
  vi.stubGlobal("WebSocket", FakeWS)
  setVisibility("visible")
  noteServerValidated()
})

afterEach(() => {
  // Disposed, never merely closed: a socket left with its wake listeners on the
  // shared window would answer the next test's events.
  for (const sock of built.splice(0)) sock.dispose()
  clearServerValidated()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

// `pagehide` closes a PTY socket deliberately (a page holding one open is
// evicted from the bfcache anyway, and the server keeps a phantom owner until
// its next send fails). The return that follows is not always a `pageshow`.
describe("a PTY socket closed by the page lifecycle", () => {
  it.each([
    ["focus", () => window.dispatchEvent(new Event("focus"))],
    ["online", () => window.dispatchEvent(new Event("online"))],
    ["pageshow", () => window.dispatchEvent(new Event("pageshow"))],
    [
      "visibilitychange",
      () => document.dispatchEvent(new Event("visibilitychange")),
    ],
  ])("comes back on a bare %s", (_name, fire) => {
    const sock = socket()
    sock.connect()
    FakeWS.instances.at(-1)!.open()
    sock.close()
    fire()
    expect(FakeWS.instances).toHaveLength(2)
  })

  // The gate's own wake is the other way back, and it used to be unsubscribed by
  // `close()` along with everything else.
  it("comes back when the run-identity gate opens", () => {
    const sock = socket()
    sock.connect()
    FakeWS.instances.at(-1)!.open()
    sock.close()
    clearServerValidated()
    noteServerValidated()
    expect(FakeWS.instances).toHaveLength(2)
  })
})

describe("a disposed PTY socket", () => {
  it("answers no wake signal and no gate opening, ever", () => {
    const sock = socket()
    sock.connect()
    FakeWS.instances.at(-1)!.open()
    sock.dispose()
    window.dispatchEvent(new Event("focus"))
    window.dispatchEvent(new Event("online"))
    window.dispatchEvent(new Event("pageshow"))
    document.dispatchEvent(new Event("visibilitychange"))
    clearServerValidated()
    noteServerValidated()
    vi.advanceTimersByTime(600_000)
    expect(FakeWS.instances).toHaveLength(1)
  })
})
