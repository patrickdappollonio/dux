// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"

import { REDUCED_MOTION_QUERY } from "@/hooks/use-reduced-motion"
import { scrollFocusedTabIntoView } from "./tabFocusScroll"

// jsdom never matches :focus-visible, so keyboard focus is stubbed. A browser
// check must confirm it: Tab or arrow onto a tab at the far end of a long strip
// and the strip scrolls until that tab, ring included, is fully in view.
function strip(keyboard: boolean) {
  const tab = document.createElement("div")
  tab.setAttribute("role", "tab")
  const menu = document.createElement("button")
  tab.append(menu)
  document.body.append(tab)
  const scroll = vi.fn()
  tab.scrollIntoView = scroll
  const matches = Element.prototype.matches
  vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, s: string) {
    return s === ":focus-visible" ? keyboard : matches.call(this, s)
  })
  return { tab, menu, scroll }
}

function reducedMotion(on: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: on && query === REDUCED_MOTION_QUERY }))
}

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  document.body.replaceChildren()
})

describe("scrollFocusedTabIntoView", () => {
  it("scrolls a tab taking keyboard focus fully into view, smoothly", () => {
    reducedMotion(false)
    const { tab, scroll } = strip(true)
    scrollFocusedTabIntoView(tab)
    expect(scroll).toHaveBeenCalledWith({ block: "nearest", inline: "nearest", behavior: "smooth" })
  })

  it("scrolls the tab whose ⋯ takes keyboard focus", () => {
    reducedMotion(false)
    const { menu, scroll } = strip(true)
    scrollFocusedTabIntoView(menu)
    expect(scroll).toHaveBeenCalledTimes(1)
  })

  it("jumps rather than glides under reduced motion", () => {
    reducedMotion(true)
    const { tab, scroll } = strip(true)
    scrollFocusedTabIntoView(tab)
    expect(scroll).toHaveBeenCalledWith({ block: "nearest", inline: "nearest", behavior: "auto" })
  })

  it("leaves the strip alone for a mouse click", () => {
    reducedMotion(false)
    const { tab, scroll } = strip(false)
    scrollFocusedTabIntoView(tab)
    expect(scroll).not.toHaveBeenCalled()
  })
})
