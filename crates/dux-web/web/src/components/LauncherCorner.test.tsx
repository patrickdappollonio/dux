// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { REDUCED_MOTION_QUERY } from "@/hooks/use-reduced-motion"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

const openNewLauncher = vi.fn()
vi.mock("@/lib/store", () => ({
  openNewLauncher: () => openNewLauncher(),
}))

import { LauncherCorner } from "@/components/LauncherCorner"

// The word on screen: the one label the cycle marks as shown.
const shownWord = () =>
  document.querySelector("[data-shown]")?.textContent ?? null
const button = () => screen.getByRole("button", { name: "New…" })
const advance = (ms: number) => act(() => void vi.advanceTimersByTime(ms))

let media: MatchMediaStub
beforeEach(() => {
  media = stubMatchMedia()
  vi.useFakeTimers()
  vi.clearAllMocks()
})
afterEach(() => {
  cleanup()
  media.restore()
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe("LauncherCorner", () => {
  it("opens the New dialog", () => {
    render(<LauncherCorner />)
    fireEvent.click(button())
    expect(openNewLauncher).toHaveBeenCalledOnce()
  })

  it("rests on New… and then names each kind in turn, keeping its name", () => {
    render(<LauncherCorner />)
    expect(shownWord()).toBe("New…")
    advance(7900)
    expect(shownWord()).toBe("New…")
    advance(300)
    expect(shownWord()).toBe("New agent")
    // A screen reader hears the same name whatever word is showing.
    expect(button()).toBeTruthy()
    advance(2625)
    expect(shownWord()).toBe("New terminal")
    advance(2625)
    expect(shownWord()).toBe("New project")
    advance(2625)
    expect(shownWord()).toBe("New…")
  })

  it("holds on New… while hovered, and starts again from the rest after", () => {
    render(<LauncherCorner />)
    advance(8200)
    expect(shownWord()).toBe("New agent")
    fireEvent.pointerEnter(button())
    expect(shownWord()).toBe("New…")
    advance(20000)
    expect(shownWord()).toBe("New…")
    fireEvent.pointerLeave(button())
    advance(7900)
    expect(shownWord()).toBe("New…")
    advance(300)
    expect(shownWord()).toBe("New agent")
  })

  it("holds on New… while it has keyboard focus", () => {
    // jsdom has no notion of how focus arrived, so the browser's answer to
    // ":focus-visible" is supplied: this focus came from the keyboard.
    const matches = Element.prototype.matches
    vi.spyOn(Element.prototype, "matches").mockImplementation(function (
      this: Element,
      selector: string,
    ) {
      return selector === ":focus-visible" || matches.call(this, selector)
    })
    render(<LauncherCorner />)
    act(() => button().focus())
    advance(8200)
    expect(shownWord()).toBe("New…")
    act(() => button().blur())
    advance(8200)
    expect(shownWord()).toBe("New agent")
  })

  it("never cycles under reduced motion", () => {
    media.set(REDUCED_MOTION_QUERY, true)
    render(<LauncherCorner />)
    advance(40000)
    expect(shownWord()).toBe("New…")
  })
})
