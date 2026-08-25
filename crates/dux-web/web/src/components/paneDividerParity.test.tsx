// @vitest-environment jsdom
//
// The workspace has two draggable dividers and they must feel the same. The
// left one is dux's own (the sidebar's width is a CSS variable, not a panel in
// a layout group); the right one is react-resizable-panels'. They drifted once,
// and a finger could move one and not the other. This file is what makes that
// drift a failing test rather than a bug report from a tablet.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// The store boots on import: it reads localStorage and fires the bootstrap
// fetch. jsdom exposes neither as a bare global, so stub them before the
// components (and the store behind them) load.
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
// The panel library observes its panels; jsdom ships no ResizeObserver.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
)
// A finger is the pointer throughout this file, so both dividers offer their
// 20px band. dux's hook asks for "(pointer: coarse)" and the library asks for
// "(pointer:coarse)"; the library also memoizes its answer for the life of the
// module, which is why this is stubbed once for the whole file rather than
// flipped per test.
vi.stubGlobal(
  "matchMedia",
  vi.fn((query: string) => ({
    matches: query.includes("coarse"),
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })),
)

const { SidebarProvider } = await import("@/components/ui/sidebar")
const { AppSidebar } = await import("./Sidebar")
const { ResizableHandle, ResizablePanel, ResizablePanelGroup } = await import(
  "@/components/ui/resizable"
)
const { DIVIDER_CHROME, DIVIDER_TARGET_MIN } = await import("@/lib/paneDivider")
const { sidebarWidthToPx } = await import("@/lib/sidebarResize")

const mockState = {
  spine: null,
  bootstrap: { title: "dux", dux_version: "v0" },
  selectedTarget: null,
  pendingSessionOrder: null,
  pendingProjectOrder: null,
  sidebarWidth: "18rem",
} as unknown as DuxState

afterEach(cleanup)

function sidebarDivider(): HTMLElement {
  const { container } = render(
    <SidebarProvider>
      <AppSidebar />
    </SidebarProvider>,
  )
  return container.querySelector('[data-sidebar="resize-handle"]')!
}

function changesDivider(): HTMLElement {
  const { container } = render(
    <ResizablePanelGroup orientation="horizontal">
      <ResizablePanel id="a" defaultSize="70%" />
      <ResizableHandle />
      <ResizablePanel id="b" defaultSize="30%" />
    </ResizablePanelGroup>,
  )
  return container.querySelector('[data-slot="resizable-handle"]')!
}

describe("the two workspace dividers", () => {
  it("wear the same chrome, token for token", () => {
    const left = sidebarDivider().className.split(/\s+/)
    const right = changesDivider().className.split(/\s+/)
    for (const token of DIVIDER_CHROME.split(/\s+/)) {
      expect(left, `sidebar divider is missing ${token}`).toContain(token)
      expect(right, `Changes divider is missing ${token}`).toContain(token)
    }
  })

  it("both suppress touch-action, so neither gesture is stolen as a page pan", () => {
    expect(sidebarDivider().className).toContain("touch-none")
    // The library also writes it inline, which is what actually reaches the
    // browser for the element itself; the class covers the grab band around it.
    expect(changesDivider().style.touchAction).toBe("none")
  })

  it("offer the same grab band on the same pointer kinds", () => {
    for (const el of [sidebarDivider(), changesDivider()]) {
      expect(el.className).toContain(`after:w-[${DIVIDER_TARGET_MIN.fine}px]`)
      expect(el.className).toContain(
        `pointer-coarse:after:w-[${DIVIDER_TARGET_MIN.coarse}px]`,
      )
    }
  })

  it("are both reachable and announced as separators", () => {
    for (const el of [sidebarDivider(), changesDivider()]) {
      expect(el.getAttribute("role")).toBe("separator")
      expect(el.getAttribute("aria-orientation")).toBe("vertical")
      expect(el.tabIndex).toBe(0)
    }
  })
})

// ── The gesture itself ─────────────────────────────────────────────────────
// The class assertions above pin what the two dividers look like. These pin
// what they DO, which is the half that actually broke: the sidebar's edge used
// to acquire a press by hit-testing a 4px element and then move to the absolute
// pointer position, so a finger that landed off the line either missed it
// entirely or yanked the divider sideways before moving at all.
//
// jsdom has no layout, so both dividers are handed one. The panel library reads
// `getBoundingClientRect` for its hit regions and `offsetWidth` for the group's
// size, and dux's hook reads `getBoundingClientRect` for its band; all three
// are answered from the table below.
const LAYOUT: Record<string, [number, number]> = {
  // The panel group, laid out so its separator sits at the same x as the
  // sidebar's edge would be at the default 18rem. Both dividers are then
  // pressed and dragged with the same numbers.
  group: [0, 1000],
  a: [0, 288],
  separator: [287, 288],
  b: [288, 1000],
  // The sidebar's edge at its default 18rem width.
  sidebar: [287, 288],
}

const EDGE_CENTRE = 287.5
const OFF_CENTRE = 9
const DRAG_BY = 50

function layoutKey(el: HTMLElement): string | null {
  if (el.dataset.sidebar === "resize-handle") return "sidebar"
  if (el.dataset.slot === "resizable-panel-group") return "group"
  if (el.hasAttribute("data-separator")) return "separator"
  if (el.hasAttribute("data-panel")) return el.id
  return null
}

describe("both dividers, driven by a finger", () => {
  const realRect = HTMLElement.prototype.getBoundingClientRect
  const realOffsetWidth = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "offsetWidth",
  )

  beforeEach(() => {
    HTMLElement.prototype.getBoundingClientRect = function () {
      const key = layoutKey(this)
      const span = key === null ? undefined : LAYOUT[key]
      if (!span) return realRect.call(this)
      return new DOMRect(span[0], 0, span[1] - span[0], 600)
    }
    Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
      configurable: true,
      get(this: HTMLElement) {
        const key = layoutKey(this)
        const span = key === null ? undefined : LAYOUT[key]
        return span ? span[1] - span[0] : 0
      },
    })
  })

  afterEach(() => {
    HTMLElement.prototype.getBoundingClientRect = realRect
    if (realOffsetWidth) {
      Object.defineProperty(
        HTMLElement.prototype,
        "offsetWidth",
        realOffsetWidth,
      )
    } else {
      delete (HTMLElement.prototype as { offsetWidth?: number }).offsetWidth
    }
  })

  function press(target: HTMLElement, clientX: number) {
    act(() => {
      fireEvent.pointerDown(target, {
        pointerId: 1,
        pointerType: "touch",
        clientX,
        clientY: 300,
        isPrimary: true,
      })
    })
  }

  function move(clientX: number) {
    act(() => {
      fireEvent.pointerMove(document, {
        pointerId: 1,
        pointerType: "touch",
        clientX,
        clientY: 300,
        isPrimary: true,
        buttons: 1,
      })
    })
  }

  function release(clientX: number) {
    act(() => {
      fireEvent.pointerUp(document, {
        pointerId: 1,
        pointerType: "touch",
        clientX,
        clientY: 300,
        isPrimary: true,
      })
    })
  }

  // Press `OFF_CENTRE` to the right of the line, then drag `DRAG_BY` further
  // right, and report where each divider ended up in pixels.
  function dragSidebar(): number {
    localStorage.removeItem("dux:sidebar-width")
    const handle = sidebarDivider()
    handle.setPointerCapture = () => {}
    press(handle, EDGE_CENTRE + OFF_CENTRE)
    move(EDGE_CENTRE + OFF_CENTRE + DRAG_BY)
    // Released where the move left it. `useDux` is a fixture here, so the live
    // width never reaches the DOM; the width written on release is what the
    // gesture decided.
    release(EDGE_CENTRE + OFF_CENTRE + DRAG_BY)
    return sidebarWidthToPx(localStorage.getItem("dux:sidebar-width") ?? "0")
  }

  function dragChanges(): number {
    let percent = 0
    const { container } = render(
      <ResizablePanelGroup
        orientation="horizontal"
        onLayoutChange={(l) => {
          percent = l["a"] ?? percent
        }}
      >
        <ResizablePanel id="a" defaultSize="28.8%" />
        <ResizableHandle />
        <ResizablePanel id="b" defaultSize="71.2%" />
      </ResizablePanelGroup>,
    )
    const handle = container.querySelector(
      '[data-slot="resizable-handle"]',
    ) as HTMLElement
    handle.setPointerCapture = () => {}
    press(handle, EDGE_CENTRE + OFF_CENTRE)
    move(EDGE_CENTRE + OFF_CENTRE + DRAG_BY)
    release(EDGE_CENTRE + OFF_CENTRE + DRAG_BY)
    return (percent / 100) * (LAYOUT.group[1] - LAYOUT.group[0])
  }

  // A press 9px off the line is outside a 4px element and inside the 20px band
  // both dividers claim. Neither may miss it.
  it("both acquire a press well off the painted line", () => {
    expect(dragSidebar()).not.toBe(288)
    cleanup()
    expect(dragChanges()).not.toBe(288)
  })

  // And the move that follows is worth exactly its own distance. A divider that
  // snapped to the pointer would land on the press point plus the drag, 9px
  // further along, which is the jump the old sidebar edge had.
  it("neither jumps to the pointer on the first move", () => {
    expect(dragSidebar()).toBe(288 + DRAG_BY)
    cleanup()
    expect(dragChanges()).toBeCloseTo(288 + DRAG_BY, 5)
  })
})
