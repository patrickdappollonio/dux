// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { Toaster } from "@/components/ui/sonner"

import { notifySuccess, setStatusClearSeconds } from "./notify"

// sonner sizes an expanded (hovered) toast to the height it measured, so a toast
// whose text grows after that measure shrinks under the pointer and clips its
// text. jsdom lays nothing out, so the toast's laid-out height is stood in for
// here: `natural` is what it measures with its height released, and the content
// observer is fired by hand where a browser would fire it on a resize.

let natural = 0
let observers: { fire: () => void }[] = []

beforeEach(() => {
  natural = 60
  observers = []
  vi.stubGlobal(
    "ResizeObserver",
    class {
      private readonly callback: ResizeObserverCallback
      constructor(callback: ResizeObserverCallback) {
        this.callback = callback
        observers.push({ fire: () => this.callback([], this as unknown as ResizeObserver) })
      }
      observe() {}
      disconnect() {}
    },
  )
  const original = HTMLElement.prototype.getBoundingClientRect
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (!this.hasAttribute("data-sonner-toast")) return original.call(this)
    const fixed = parseFloat(this.style.height)
    const height = Number.isFinite(fixed) ? fixed : natural
    return { x: 0, y: 0, top: 0, left: 0, right: 356, bottom: height, width: 356, height, toJSON() {} }
  })
  // Drawn at its own size: the width on screen is its laid-out width.
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(356)
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  setStatusClearSeconds(undefined)
  cleanup()
})

function toastEl(): HTMLElement {
  const el = document.querySelector<HTMLElement>("[data-sonner-toast]")
  if (!el) throw new Error("no toast rendered")
  return el
}

describe("a toast whose text changes height after sonner measured it", () => {
  it("has sonner measure it again, so a hovered toast keeps its full height", async () => {
    render(<Toaster />)
    act(() => notifySuccess("Saved image.png and sent its path."))
    await screen.findByText("Saved image.png and sent its path.")
    await waitFor(() => expect(toastEl().style.getPropertyValue("--initial-height")).toBe("60px"))
    const before = toastEl()

    // A name chip in the text switched to wrapping after sonner's measure.
    natural = 100
    act(() => observers.forEach((o) => o.fire()))

    await waitFor(() => expect(toastEl().style.getPropertyValue("--initial-height")).toBe("100px"))
    // Measured again in place: the same toast, not a second one or a remount.
    expect(document.querySelectorAll("[data-sonner-toast]")).toHaveLength(1)
    expect(toastEl()).toBe(before)
  })
})
