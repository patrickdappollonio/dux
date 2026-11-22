// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

vi.mock("@/components/MobileShell", () => ({
  MobileShell: () => <div data-testid="mobile-shell" />,
}))
vi.mock("@/lib/store", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/store")>()),
  useDux: () => ({ mobileScreen: "terminal" }) as unknown as DuxState,
}))

let media: MatchMediaStub | null = null
vi.stubGlobal("fetch", vi.fn(() => new Promise(() => {})))
media = stubMatchMedia()
const { MobileApp } = await import("./App")

// A visual viewport whose listeners can be counted; jsdom has none.
function stubVisualViewport() {
  const vv = Object.assign(new EventTarget(), {
    width: 384,
    height: 426.3,
    offsetTop: 0,
    offsetLeft: 0,
    pageTop: 0,
    scale: 1,
  })
  const add = vi.spyOn(vv, "addEventListener")
  vi.stubGlobal("visualViewport", vv)
  return add
}

beforeEach(() => {
  media ??= stubMatchMedia()
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  window.history.replaceState(null, "", "/")
})

describe("the phone shell's viewport readout", () => {
  it("mounts nothing and listens to nothing more without the switch", () => {
    const vvAdd = stubVisualViewport()
    const winAdd = vi.spyOn(window, "addEventListener")
    const interval = vi.spyOn(window, "setInterval")
    window.history.replaceState(null, "", "/#/agent/s1")
    const { container } = render(<MobileApp />)
    expect(container.querySelector("[data-viewport-debug]")).toBeNull()
    // The shell's own height hook is the only visual viewport subscriber.
    expect(vvAdd.mock.calls.map((c) => c[0]).sort()).toEqual(["resize", "scroll"])
    expect(winAdd.mock.calls.filter((c) => c[0] === "resize")).toEqual([])
    expect(interval).not.toHaveBeenCalled()
  })

  it("shows the panel, the bands and the bottom line with ?debug=viewport", () => {
    stubVisualViewport()
    window.history.replaceState(null, "", "/?debug=viewport#/agent/s1")
    const { container } = render(<MobileApp />)
    expect(container.querySelector("[data-viewport-debug='bands']")?.children).toHaveLength(10)
    expect(container.querySelector("[data-viewport-debug='vv-bottom']")).not.toBeNull()
    expect(container.querySelector("[data-viewport-debug='panel']")?.textContent).toContain(
      "hook 426 live 426 diff 0",
    )
  })
})
