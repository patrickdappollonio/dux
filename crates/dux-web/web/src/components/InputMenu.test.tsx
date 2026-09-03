// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { InputMenuGates } from "@/lib/inputMenu"

// The terminal-keys preference is read through the store and written through
// it, so the store is stubbed at both ends: a settable state for the label
// (Hide vs Show) and a spy for the write.
const setAccessoryBarVisibility = vi.fn()
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    setAccessoryBarVisibility: (...a: unknown[]) =>
      setAccessoryBarVisibility(...a),
    exitTheater: (...a: unknown[]) => exitTheater(...a),
  }
})
const exitTheater = vi.fn()
const setTypingSurface = vi.fn()
vi.mock("@/lib/typingSurface", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/typingSurface")>()
  return { ...actual, setTypingSurface: (...a: unknown[]) => setTypingSurface(...a) }
})

// The store reads storage and the network at import time, and the menu
// primitive asks for a media query; stub all three before the import below.
const mem = new Map<string, string>()
vi.stubGlobal("localStorage", {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, String(v)),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})
vi.stubGlobal(
  "fetch",
  vi.fn(() => Promise.reject(new Error("no network in tests"))),
)
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

const { InputMenu } = await import("./InputMenu")
const { inputMenuHasItems } = await import("@/lib/inputMenu")

const ALL_OFF: InputMenuGates = {
  attach: false,
  surfaceSwitch: false,
  keysToggle: false,
  theaterExit: false,
}

function state(keys = true): DuxState {
  return {
    mobileAccessoryBarOverride: keys,
    bootstrap: null,
  } as unknown as DuxState
}

function open(
  gates: Partial<InputMenuGates>,
  props: { onAttach?: () => void; composeSurface?: boolean } = {},
) {
  render(<InputMenu gates={{ ...ALL_OFF, ...gates }} {...props} />)
  const trigger = screen.queryByRole("button", { name: "Input options" })
  if (trigger) fireEvent.click(trigger)
  return trigger
}

beforeEach(() => {
  mockState = state()
  setAccessoryBarVisibility.mockClear()
  setTypingSurface.mockClear()
})
afterEach(() => cleanup())

describe("inputMenuHasItems", () => {
  // The trigger asks this before it renders, because an ⋯ that opens an empty
  // popup is worse than no ⋯, and the empty state is genuinely reachable.
  it("is false only when every gate is off", () => {
    expect(inputMenuHasItems(ALL_OFF)).toBe(false)
    for (const key of Object.keys(ALL_OFF) as (keyof InputMenuGates)[]) {
      expect(inputMenuHasItems({ ...ALL_OFF, [key]: true })).toBe(true)
    }
  })
})

describe("InputMenu", () => {
  it("renders no trigger at all when there is nothing to put in it", () => {
    expect(open({})).toBeNull()
  })

  it("offers Attach a file… only on its own gate, and calls back", () => {
    const onAttach = vi.fn()
    open({ attach: true }, { onAttach })
    // The trailing ellipsis marks the item as opening a dialog: here the
    // operating system's own file picker.
    fireEvent.click(screen.getByText("Attach a file…"))
    expect(onAttach).toHaveBeenCalledTimes(1)
  })

  it("carries the way out of theater, and only while theater is on", () => {
    // The input ⋯ renders in EVERY bar state, which is what makes it the exit
    // that still works on a phone whose floating pill is under the keyboard.
    open({ keysToggle: true })
    expect(screen.queryByText("Leave theater mode")).toBeNull()
    cleanup()
    open({ theaterExit: true })
    fireEvent.click(screen.getByText("Leave theater mode"))
    expect(exitTheater).toHaveBeenCalledTimes(1)
  })

  it("hides Attach a file… when the caller says so", () => {
    open({ keysToggle: true })
    expect(screen.queryByText("Attach a file…")).toBeNull()
  })

  it("names the typing-surface switch after what it does, both ways", () => {
    open({ surfaceSwitch: true }, { composeSurface: true })
    fireEvent.click(screen.getByText("Type directly in the terminal"))
    expect(setTypingSurface).toHaveBeenCalledExactlyOnceWith("direct")
    cleanup()
    setTypingSurface.mockClear()
    open({ surfaceSwitch: true }, { composeSurface: false })
    fireEvent.click(screen.getByText("Use the message box"))
    expect(setTypingSurface).toHaveBeenCalledExactlyOnceWith("compose")
  })

  // Selecting an item closes the menu, so each write gets its own open.
  it("flips the keys toggle's label and write with the bar's own state", () => {
    const pick = (label: string, visible: boolean) => {
      cleanup()
      setAccessoryBarVisibility.mockClear()
      mockState = state(visible)
      open({ keysToggle: true })
      fireEvent.click(screen.getByText(label))
    }
    pick("Hide terminal keys", true)
    expect(setAccessoryBarVisibility).toHaveBeenCalledExactlyOnceWith(false)
    pick("Show terminal keys", false)
    expect(setAccessoryBarVisibility).toHaveBeenCalledExactlyOnceWith(true)
  })

  // The top bar has no toggle here any more: theater mode is the one way to
  // hide the phone's chrome, so no gate and no label may bring a second one
  // back.
  it("carries no top-bar toggle whatever the caller asks for", () => {
    open({ attach: true, surfaceSwitch: true, keysToggle: true, theaterExit: true })
    expect(screen.queryByText("Hide top bar")).toBeNull()
    expect(screen.queryByText("Show top bar")).toBeNull()
  })

  // Visibility is the CALLER's: the component gates nothing for itself, which
  // is what lets the input ⋯ widen the keys item to a coarse-pointer tablet
  // while the phone header menus keep their own narrower gate.
  it("renders exactly the items the caller asked for", () => {
    open({ attach: true, surfaceSwitch: true }, { composeSurface: false })
    expect(screen.getByText("Attach a file…")).toBeTruthy()
    expect(screen.getByText("Use the message box")).toBeTruthy()
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
  })
})
