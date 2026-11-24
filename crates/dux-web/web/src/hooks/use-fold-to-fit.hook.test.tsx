// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react"
import { useEffect, useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { FOLD_SLACK_PX, useFoldHold, useFoldToFit, FoldHoldContext } from "./use-fold-to-fit"

// jsdom lays out nothing, so the header is laid out by hand: the names need
// NAMES in full, each control stands at its width in `widths`, and `room` is the
// row the names have with every control folded.
const NAMES = 300
const ITEMS = ["a", "b"]
let room = 900
let widths: Record<string, number> = {}
let resizeRow: (() => void)[] = []
let fonts: EventTarget

let setHeld: (open: boolean) => void = () => {}
let setHolderMounted: (mounted: boolean) => void = () => {}
// What the holder's popup does when its close has finished animating.
let settleHeld: () => void = () => {}
let settlesAtOnce = true

function Probe() {
  const { isFolded, rowRef, itemRef, hold } = useFoldToFit({
    enabled: true,
    items: ITEMS,
    watch: "names",
  })
  const [holderMounted, setMounted] = useState(true)
  useEffect(() => {
    setHolderMounted = setMounted
  }, [])
  return (
    <FoldHoldContext.Provider value={hold}>
      <div>
        <div ref={rowRef} data-row>
          <span data-chip data-fold-name>
            names
          </span>
        </div>
        {ITEMS.filter((i) => !isFolded(i)).map((i) => (
          <span key={i} ref={itemRef(i)} data-item={i} />
        ))}
        {holderMounted ? <Holder /> : null}
      </div>
    </FoldHoldContext.Provider>
  )
}

function Holder() {
  const [open, setOpen] = useState(false)
  const settle = useFoldHold(open)
  useEffect(() => {
    setHeld = setOpen
    settleHeld = settle
    if (!open && settlesAtOnce) settle()
  }, [open, settle])
  return null
}

function standingItems(): string[] {
  return [...document.querySelectorAll("[data-item]")].map((el) => el.getAttribute("data-item")!)
}
function rowWidth(): number {
  return standingItems().reduce((w, i) => w - widths[i], room)
}

beforeEach(() => {
  room = 900
  widths = { a: 118, b: 118 }
  resizeRow = []
  settlesAtOnce = true
  fonts = new EventTarget()
  Object.defineProperty(document, "fonts", { value: fonts, configurable: true })
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(private cb: () => void) {}
      observe(el: Element) {
        if (el.hasAttribute("data-row")) resizeRow.push(this.cb)
      }
      unobserve() {}
      disconnect() {}
    },
  )
  vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockImplementation(function (this: HTMLElement) {
    return this.hasAttribute("data-fold-name") ? NAMES : 0
  })
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (this: HTMLElement) {
    if (this.hasAttribute("data-fold-name")) return Math.min(NAMES, rowWidth())
    return this.hasAttribute("data-row") ? rowWidth() : 0
  })
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    const item = this.getAttribute("data-item")
    const width = item ? widths[item] : this.hasAttribute("data-chip") ? Math.min(NAMES, rowWidth()) : 0
    return { left: 0, right: width, width, top: 0, bottom: 0, height: 0, x: 0, y: 0 } as DOMRect
  })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  Reflect.deleteProperty(document, "fonts")
})

// The hold a popup's close leaves behind ends a task after it is settled.
async function settled(settle: () => void) {
  await act(async () => {
    settle()
    await new Promise((r) => setTimeout(r, 0))
  })
}

function resizeTo(width: number) {
  room = width
  act(() => resizeRow.forEach((cb) => cb()))
}
function fontChangesWidthsTo(width: number) {
  widths = { a: width, b: width }
  act(() => {
    fonts.dispatchEvent(new Event("loadingdone"))
  })
}

describe("useFoldToFit with a control whose width changes while it is folded", () => {
  // Two controls of 108px each beside names of 300px: both stand from 524px of
  // room, so 530px has both standing.
  it("rests in one state at a width whichever way it was reached", () => {
    // Path 1: both controls are 118px wide when the first one folds.
    render(<Probe />)
    resizeTo(530)
    expect(standingItems()).toEqual(["b"])
    // The font arrives and makes them 108px; the folded one is not on screen.
    fontChangesWidthsTo(108)
    resizeTo(530)
    const viaFold = standingItems()
    cleanup()

    // Path 2: 108px from the start.
    widths = { a: 108, b: 108 }
    resizeRow = []
    render(<Probe />)
    resizeTo(400)
    resizeTo(900)
    resizeTo(530)
    expect(standingItems()).toEqual(["a", "b"])
    expect(viaFold).toEqual(standingItems())
  })

  it("is as current for a control that stays folded through the font change", () => {
    render(<Probe />)
    resizeTo(530)
    expect(standingItems()).toEqual(["b"])
    fontChangesWidthsTo(108)
    expect(standingItems()).toEqual(["a", "b"])
  })

  it("re-measures after a zoom, which changes widths without changing the row", () => {
    Object.defineProperty(window, "devicePixelRatio", { value: 1, configurable: true })
    render(<Probe />)
    resizeTo(530)
    expect(standingItems()).toEqual(["b"])
    widths = { a: 108, b: 108 }
    Object.defineProperty(window, "devicePixelRatio", { value: 1.25, configurable: true })
    act(() => {
      window.dispatchEvent(new Event("resize"))
    })
    expect(standingItems()).toEqual(["a", "b"])
    Reflect.deleteProperty(window, "devicePixelRatio")
  })
})

describe("useFoldToFit while something a control opened is open", () => {
  it("changes nothing across a fold boundary, and settles once it closes", async () => {
    widths = { a: 108, b: 108 }
    render(<Probe />)
    expect(standingItems()).toEqual(["a", "b"])
    act(() => setHeld(true))
    resizeTo(530 - 30)
    resizeTo(300)
    expect(standingItems()).toEqual(["a", "b"])
    resizeTo(900)
    resizeTo(300)
    expect(standingItems()).toEqual(["a", "b"])
    act(() => setHeld(false))
    await act(() => new Promise((r) => setTimeout(r, 0)))
    expect(standingItems()).toEqual([])
    resizeTo(900)
    expect(standingItems()).toEqual(["a", "b"])
    expect(FOLD_SLACK_PX).toBeGreaterThan(0)
  })

  it("applies a font change that arrived while held once it closes", async () => {
    render(<Probe />)
    resizeTo(530)
    expect(standingItems()).toEqual(["b"])
    act(() => setHeld(true))
    fontChangesWidthsTo(108)
    expect(standingItems()).toEqual(["b"])
    act(() => setHeld(false))
    await act(() => new Promise((r) => setTimeout(r, 0)))
    expect(standingItems()).toEqual(["a", "b"])
  })

  it("keeps holding after the popup closes until its close has finished", async () => {
    widths = { a: 108, b: 108 }
    settlesAtOnce = false
    render(<Probe />)
    act(() => setHeld(true))
    resizeTo(300)
    act(() => setHeld(false))
    expect(standingItems()).toEqual(["a", "b"])
    await settled(settleHeld)
    expect(standingItems()).toEqual([])
  })

  it("lets go when the holder unmounts while open", () => {
    widths = { a: 108, b: 108 }
    render(<Probe />)
    act(() => setHeld(true))
    resizeTo(300)
    expect(standingItems()).toEqual(["a", "b"])
    act(() => setHolderMounted(false))
    expect(standingItems()).toEqual([])
  })

  it("settles on the close it was told of, not an earlier close finishing late", async () => {
    widths = { a: 108, b: 108 }
    settlesAtOnce = false
    render(<Probe />)
    act(() => setHeld(true))
    const first = settleHeld
    resizeTo(300)
    // Opened again before the first close finished.
    act(() => setHeld(false))
    act(() => setHeld(true))
    await settled(first)
    expect(standingItems()).toEqual(["a", "b"])
    act(() => setHeld(false))
    await settled(settleHeld)
    expect(standingItems()).toEqual([])
  })
})
