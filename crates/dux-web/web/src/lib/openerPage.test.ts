// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

type Opener = typeof import("./openerPage")
type Seat = typeof import("./pageSeat")

const OPENER_PAGE = "c".repeat(32)

let opener: Opener
let seat: Seat

function otherWindow(): Window {
  const frame = document.createElement("iframe")
  document.body.append(frame)
  return frame.contentWindow as Window
}

function deliver(data: unknown, source: Window, origin = window.location.origin): void {
  window.dispatchEvent(new MessageEvent("message", { data, origin, source }))
}

beforeEach(async () => {
  vi.resetModules()
  opener = await import("./openerPage")
  seat = await import("./pageSeat")
})

afterEach(() => {
  vi.unstubAllGlobals()
  document.body.replaceChildren()
})

describe("a page answering the tabs it opened", () => {
  it("tells a tab it opened which pages it acts as, and nobody else", () => {
    const stop = opener.answerOpenedTabs()
    const opened = otherWindow()
    Object.defineProperty(opened, "opener", { value: window })
    const told = vi.spyOn(opened, "postMessage").mockImplementation(() => {})
    const stranger = otherWindow()
    const strangerTold = vi.spyOn(stranger, "postMessage").mockImplementation(() => {})

    deliver({ type: "dux:pages?" }, opened, "https://elsewhere.example")
    deliver({ type: "something-else" }, opened)
    deliver({ type: "dux:pages?" }, stranger)
    expect(told).not.toHaveBeenCalled()
    expect(strangerTold).not.toHaveBeenCalled()

    deliver({ type: "dux:pages?" }, opened)
    expect(told).toHaveBeenCalledExactlyOnceWith(
      { type: "dux:pages", pages: [seat.pageSeatId()] },
      window.location.origin,
    )
    stop()
  })
})

describe("a tab asking the page that opened it", () => {
  beforeEach(() => seat.setEditorSurface(true))

  it("is handed nothing when no page opened it", async () => {
    expect(opener.hasOpener()).toBe(false)
    expect(await opener.askOpenerForPages()).toBe(false)
    expect(seat.actsForOpener()).toBe(false)
  })

  it("asks only its opener, at this origin, and takes only its opener's answer", async () => {
    const from = otherWindow()
    const asked = vi.spyOn(from, "postMessage").mockImplementation(() => {})
    vi.stubGlobal("opener", from)

    const answer = opener.askOpenerForPages()
    expect(asked).toHaveBeenCalledExactlyOnceWith({ type: "dux:pages?" }, window.location.origin)
    const tell = { type: "dux:pages", pages: [OPENER_PAGE] }
    deliver(tell, otherWindow())
    deliver(tell, from, "https://elsewhere.example")
    expect(seat.actsForOpener()).toBe(false)
    deliver(tell, from)
    expect(await answer).toBe(true)
    expect(seat.actingPageIds()).toEqual([seat.pageSeatId(), OPENER_PAGE])
  })

  it("goes on without an opener that never answers", async () => {
    vi.useFakeTimers()
    const from = otherWindow()
    vi.spyOn(from, "postMessage").mockImplementation(() => {})
    vi.stubGlobal("opener", from)
    const answer = opener.askOpenerForPages()
    await vi.advanceTimersByTimeAsync(2000)
    expect(await answer).toBe(false)
    vi.useRealTimers()
  })
})
