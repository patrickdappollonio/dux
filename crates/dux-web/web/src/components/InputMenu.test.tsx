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
const switchTypingSurface = vi.fn()
const hideTerminalKeysHint = vi.fn()
vi.mock("@/lib/typingSurface", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/typingSurface")>()
  return {
    ...actual,
    switchTypingSurface: (...a: unknown[]) => switchTypingSurface(...a),
    hideTerminalKeysHint: (...a: unknown[]) => hideTerminalKeysHint(...a),
  }
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
  surfaceSwitch: false,
  keysToggle: false,
}

function state(keys = true): DuxState {
  return {
    mobileAccessoryBarOverride: keys,
    bootstrap: null,
  } as unknown as DuxState
}

function open(
  gates: Partial<InputMenuGates>,
  props: {
    composeSurface?: boolean
    directLeavesNothingBelow?: boolean
    keysHideLeavesNothingBelow?: boolean
  } = {},
) {
  render(<InputMenu gates={{ ...ALL_OFF, ...gates }} {...props} />)
  const trigger = screen.queryByRole("button", { name: "Input options" })
  if (trigger) fireEvent.click(trigger)
  return trigger
}

beforeEach(() => {
  mockState = state()
  setAccessoryBarVisibility.mockClear()
  switchTypingSurface.mockClear()
  hideTerminalKeysHint.mockClear()
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

  // NEITHER OF THESE HAS A GATE HERE ANY MORE, and that is the point: this
  // menu lives inside the virtual input and dies with it, so it can be nobody's
  // permanent home for attaching a file or for leaving theater. Both live in
  // the top menu's INPUT group instead, and there is no longer a field a caller
  // could set to put them back.
  it("carries neither the attach item nor the way out of theater", () => {
    open({ surfaceSwitch: true, keysToggle: true })
    expect(screen.queryByText("Attach a file…")).toBeNull()
    expect(screen.queryByText("Leave theater mode")).toBeNull()
    expect(exitTheater).not.toHaveBeenCalled()
  })

  // The two directions live in different menus (the way out inside the virtual
  // input, the way back in the top menu that outlives it), so the caller says
  // which one it is and the wording follows.
  it("names the typing-surface switch after what it does, both ways", () => {
    open({ surfaceSwitch: true }, { composeSurface: true })
    fireEvent.click(screen.getByText("Type directly in the terminal"))
    expect(switchTypingSurface).toHaveBeenCalledExactlyOnceWith("direct", true)
    cleanup()
    switchTypingSurface.mockClear()
    open({ surfaceSwitch: true }, { composeSurface: false })
    fireEvent.click(screen.getByText("Use virtual input"))
    expect(switchTypingSurface).toHaveBeenCalledExactlyOnceWith("compose", true)
  })

  // WHETHER THE HINT HAS ANYTHING TO SAY IS THE PANE'S ANSWER, not this menu's.
  // A key row that survives the switch keeps this very `⋯` on screen with the
  // way back inside it, and a toast sending the reader to another menu would
  // name the wrong control.
  it("carries the pane's answer about what the switch would leave behind", () => {
    open(
      { surfaceSwitch: true },
      { composeSurface: true, directLeavesNothingBelow: false },
    )
    fireEvent.click(screen.getByText("Type directly in the terminal"))
    expect(switchTypingSurface).toHaveBeenCalledExactlyOnceWith("direct", false)
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

  // THE BACK DOOR OUT OF THE VIRTUAL INPUT. From direct typing the key row is
  // the whole bottom bar, so hiding it takes this very `⋯` away; the pane says
  // whether anything is left below and the same one-time hint decides.
  it("asks for the way-back hint when hiding the keys leaves nothing", () => {
    open({ keysToggle: true }, { keysHideLeavesNothingBelow: true })
    fireEvent.click(screen.getByText("Hide terminal keys"))
    expect(setAccessoryBarVisibility).toHaveBeenCalledExactlyOnceWith(false)
    expect(hideTerminalKeysHint).toHaveBeenCalledExactlyOnceWith(true)
  })

  it("carries the pane's answer about what the keys leave behind", () => {
    open({ keysToggle: true }, { keysHideLeavesNothingBelow: false })
    fireEvent.click(screen.getByText("Hide terminal keys"))
    expect(hideTerminalKeysHint).toHaveBeenCalledExactlyOnceWith(false)
  })

  // Only ever on the way DOWN: bringing the row back can strand nobody.
  it("asks for nothing when the keys are being shown", () => {
    mockState = state(false)
    open({ keysToggle: true }, { keysHideLeavesNothingBelow: true })
    fireEvent.click(screen.getByText("Show terminal keys"))
    expect(hideTerminalKeysHint).not.toHaveBeenCalled()
  })

  // The top bar has no toggle here any more: theater mode is the one way to
  // hide the phone's chrome, so no gate and no label may bring a second one
  // back.
  it("carries no top-bar toggle whatever the caller asks for", () => {
    open({ surfaceSwitch: true, keysToggle: true })
    expect(screen.queryByText("Hide top bar")).toBeNull()
    expect(screen.queryByText("Show top bar")).toBeNull()
  })

  // Visibility is the CALLER's: the component gates nothing for itself, which
  // is what lets the input ⋯ widen the keys item to a coarse-pointer tablet
  // while the phone header menus keep their own narrower gate.
  it("renders exactly the items the caller asked for", () => {
    open({ surfaceSwitch: true }, { composeSurface: false })
    expect(screen.getByText("Use virtual input")).toBeTruthy()
    expect(screen.queryByText("Hide terminal keys")).toBeNull()
  })
})
