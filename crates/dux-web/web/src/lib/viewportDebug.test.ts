// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"
import {
  findCut,
  pushSizeEvent,
  viewportDebugEnabled,
  type SizeEvent,
} from "./viewportDebug"

describe("viewportDebugEnabled", () => {
  it("is on only for debug=viewport in the page's own query", () => {
    expect(viewportDebugEnabled("?debug=viewport")).toBe(true)
    expect(viewportDebugEnabled("?a=1&debug=viewport")).toBe(true)
    expect(viewportDebugEnabled("")).toBe(false)
    expect(viewportDebugEnabled("?debug=other")).toBe(false)
  })

  // The store rewrites only the hash, and writes path + query when it clears
  // the hash, so the switch rides every in-app move until the person deletes
  // it from the address.
  it("survives the store's own address writes", async () => {
    window.history.replaceState(null, "", "/?debug=viewport#/agent/s1")
    vi.stubGlobal("fetch", () => new Promise(() => {}))
    const { navigateUp, selectSession } = await import("./store")
    selectSession("s2")
    navigateUp()
    navigateUp()
    expect(viewportDebugEnabled(window.location.search)).toBe(true)
    window.history.replaceState(null, "", "/#/")
    expect(viewportDebugEnabled(window.location.search)).toBe(false)
  })
})

describe("findCut", () => {
  const rect = (top: number, bottom: number) => ({ top, bottom })

  it("names the nearest clipping ancestor whose bottom is above the box's", () => {
    expect(
      findCut(rect(400, 440), [
        { label: "plain", rect: rect(0, 420), clips: false },
        { label: "fits", rect: rect(0, 450), clips: true },
        { label: "shell-inner", rect: rect(0, 425), clips: true },
        { label: "shell", rect: rect(0, 410), clips: true },
      ]),
    ).toEqual({ by: "shell-inner", px: 15 })
  })

  it("is null when every clipping ancestor holds the box, sub-pixel slack included", () => {
    expect(
      findCut(rect(400, 440.3), [{ label: "shell", rect: rect(0, 440), clips: true }]),
    ).toBeNull()
  })
})

describe("pushSizeEvent", () => {
  it("keeps the newest eight, newest first", () => {
    let log: SizeEvent[] = []
    for (let i = 0; i < 10; i++) {
      log = pushSizeEvent(log, { t: i, type: "vv-resize", height: 400 + i })
    }
    expect(log).toHaveLength(8)
    expect(log[0].t).toBe(9)
    expect(log[7].t).toBe(2)
  })
})
