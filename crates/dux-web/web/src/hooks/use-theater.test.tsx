// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// The two page-wide pieces of theater mode: the ONE refit the gesture costs,
// and the Escape rule that must not steal the child's Escape.

let mockState: DuxState
const exitTheaterMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    exitTheater: (...a: unknown[]) => exitTheaterMock(...a),
  }
})

let reducedMotion = false
vi.mock("@/hooks/use-reduced-motion", () => ({
  REDUCED_MOTION_QUERY: "(prefers-reduced-motion: reduce)",
  usePrefersReducedMotion: () => reducedMotion,
}))

function installBootStubs() {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()
const { useTheaterEscape, useTheaterGesture } = await import("./use-theater")
const { layoutGestureDepth, registerLayoutGestureHolder } = await import(
  "@/lib/layoutGesture"
)
const { THEATER_TRANSITION_MS, registerTheaterTabs } = await import(
  "@/lib/theater"
)
const { InputMenu } = await import("@/components/InputMenu")

function state(theater: boolean): DuxState {
  return { theater } as unknown as DuxState
}

function Gesture() {
  useTheaterGesture()
  return null
}

function Escape() {
  useTheaterEscape()
  return null
}

beforeEach(() => {
  reducedMotion = false
  exitTheaterMock.mockReset()
  vi.useFakeTimers()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  // A failed assertion must not leave a hold behind for the rest of the file.
  while (layoutGestureDepth() > 0) {
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    break
  }
})

function pressEscape(target: EventTarget = document.body) {
  act(() => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
        cancelable: true,
      }),
    )
  })
}

describe("the one refit per toggle", () => {
  it("holds the pane for the transition and releases it exactly once", () => {
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    mockState = state(false)
    const { rerender } = render(<Gesture />)
    // Mounting is not a gesture: a page that opens in theater has no
    // transition to wait out.
    expect(pane.hold).not.toHaveBeenCalled()

    mockState = state(true)
    rerender(<Gesture />)
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).not.toHaveBeenCalled()

    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("pays the same one refit on the way back out", () => {
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    mockState = state(true)
    const { rerender } = render(<Gesture />)
    mockState = state(false)
    rerender(<Gesture />)
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS)
    })
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("re-arms the window on a second toggle instead of releasing mid-animation", () => {
    // Written as an ordinary effect cleanup, the first gesture's canceller ran
    // the moment the mode flipped again, which fitted the terminal at a
    // geometry the layout was still moving through.
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    mockState = state(false)
    const { rerender } = render(<Gesture />)
    mockState = state(true)
    rerender(<Gesture />)
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS - 50)
    })
    mockState = state(false)
    rerender(<Gesture />)
    act(() => {
      vi.advanceTimersByTime(THEATER_TRANSITION_MS - 50)
    })
    expect(pane.release).not.toHaveBeenCalled()
    act(() => {
      vi.advanceTimersByTime(50)
    })
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })

  it("lets the hold go when the page tears down mid-transition", () => {
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    mockState = state(false)
    const { rerender, unmount } = render(<Gesture />)
    mockState = state(true)
    rerender(<Gesture />)
    unmount()
    expect(pane.release).toHaveBeenCalledTimes(1)
    expect(layoutGestureDepth()).toBe(0)
    off()
  })

  it("still costs one refit under reduced motion, with nothing to wait for", () => {
    reducedMotion = true
    const pane = { hold: vi.fn(), release: vi.fn() }
    const off = registerLayoutGestureHolder(pane)
    mockState = state(false)
    const { rerender } = render(<Gesture />)
    mockState = state(true)
    rerender(<Gesture />)
    act(() => {
      vi.advanceTimersByTime(0)
    })
    expect(pane.hold).toHaveBeenCalledTimes(1)
    expect(pane.release).toHaveBeenCalledTimes(1)
    off()
  })
})

describe("Escape in theater", () => {
  it("leaves theater when focus is nowhere typeable", () => {
    mockState = state(true)
    render(<Escape />)
    act(() => {
      document.body.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      )
    })
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
  })

  it("leaves Escape to the child while a typing surface has focus", () => {
    mockState = state(true)
    render(<Escape />)
    const field = document.createElement("textarea")
    document.body.appendChild(field)
    act(() => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      )
    })
    expect(exitTheaterMock).not.toHaveBeenCalled()
    field.remove()
  })

  it("listens for nothing at all outside theater", () => {
    mockState = state(false)
    render(<Escape />)
    pressEscape()
    expect(exitTheaterMock).not.toHaveBeenCalled()
  })

  it("abstains once an open menu has answered the same press", () => {
    // The real dropdown, not a stand-in: Base UI's dismiss hook is what marks
    // the keystroke as answered, and the whole point is that dux reads ITS
    // flag. One press closes the menu and nothing else.
    mockState = state(true)
    render(
      <>
        <Escape />
        <InputMenu
          gates={{
            attach: false,
            surfaceSwitch: false,
            keysToggle: true,
            topBarToggle: false,
            theaterExit: true,
          }}
        />
      </>,
    )
    fireEvent.click(screen.getByRole("button", { name: "Input options" }))
    const item = screen.getByText("Leave theater mode")
    pressEscape(item)
    expect(screen.queryByText("Leave theater mode")).toBeNull()
    expect(exitTheaterMock).not.toHaveBeenCalled()
    // The menu is gone, so the next press is theater's.
    pressEscape()
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
  })

  it("collapses the pill's folded-out tab strip before it leaves the mode", () => {
    mockState = state(true)
    let expanded = true
    const collapse = vi.fn(() => {
      expanded = false
    })
    const off = registerTheaterTabs({ expanded: () => expanded, collapse })
    render(<Escape />)
    pressEscape()
    expect(collapse).toHaveBeenCalledTimes(1)
    expect(exitTheaterMock).not.toHaveBeenCalled()
    // The strip is away, so the next press is the exit.
    pressEscape()
    expect(exitTheaterMock).toHaveBeenCalledTimes(1)
    off()
  })
})
