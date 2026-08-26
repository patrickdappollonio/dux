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
const {
  DIVIDER_ACTIVE_PAINT,
  DIVIDER_CHROME,
  DIVIDER_DRAG_THRESHOLD_PX,
  DIVIDER_HELD_ATTR,
  DIVIDER_HELD_OFF,
  DIVIDER_HELD_ON,
  DIVIDER_STACKING,
  DIVIDER_TARGET_MIN,
} = await import("@/lib/paneDivider")
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

  // WHY STACKING IS A PARITY RULE. A divider is a sibling of the two panes it
  // separates, so a pane painted later covers its transparent grab band, and a
  // finger's press lands on the PANE. The pane's `touch-action` is `auto`, the
  // browser claims the gesture as a scroll, and the divider never moves.
  // Measured on a touch tablet: the Changes separator had no stacking of its
  // own, so a press anywhere from its line to 9px on either side hit the
  // changed-files list and closed the pane instead of dragging it.
  it("both sit above the panes they separate", () => {
    for (const el of [sidebarDivider(), changesDivider()]) {
      expect(el.className.split(/\s+/)).toContain(DIVIDER_STACKING)
    }
  })

  // `hover:` is unreachable with a finger, so a held divider looked exactly
  // like an idle one on a touch screen. Both dividers publish the held state in
  // the SAME attribute, and it is DUX'S attribute on both: the library's own
  // `data-separator` never comes back off after a cancelled touch, so a paint
  // keyed on it stayed lit with nothing on the glass.
  it("both paint a held state a finger can see, from dux's own attribute", () => {
    for (const el of [sidebarDivider(), changesDivider()]) {
      expect(el.className.split(/\s+/)).toContain(DIVIDER_ACTIVE_PAINT)
      expect(DIVIDER_ACTIVE_PAINT).toContain(DIVIDER_HELD_ATTR.slice("data-".length))
      expect(el.getAttribute(DIVIDER_HELD_ATTR)).toBe(DIVIDER_HELD_OFF)
    }
  })

  // A PRESS IS NOT A KEYBOARD ARRIVAL. Both hooks move focus to the divider on
  // pointerdown so a drag can be carried on from the keyboard, and a ring
  // painted for that press is a ring left standing beside the line under a
  // finger that has already lifted. Nothing may paint on bare `:focus`.
  it("both keep their focus ring for the keyboard only", () => {
    for (const el of [sidebarDivider(), changesDivider()]) {
      const tokens = el.className.split(/\s+/)
      expect(tokens).toContain("focus-visible:ring-1")
      expect(tokens).toContain("focus-visible:ring-ring")
      expect(tokens).toContain("focus-visible:outline-hidden")
      expect(tokens.filter((t) => t.startsWith("focus:"))).toEqual([])
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
  const realOffsetLeft = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "offsetLeft",
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
    // `offsetLeft` matters as much as the rects: react-resizable-panels 4.11.2
    // sorts a group's children by `offsetLeft` (then by `offsetWidth`) to work
    // out which panels each separator sits between. jsdom answers 0 for every
    // element, so without this the hair-thin separator sorts FIRST, the library
    // never pairs it with its panels, and it falls back to the bare gap between
    // them. The drag still works, which is why this went unnoticed; everything
    // that IDENTIFIES the separator (its focus, its `data-separator` state, its
    // aria values) silently does not.
    Object.defineProperty(HTMLElement.prototype, "offsetLeft", {
      configurable: true,
      get(this: HTMLElement) {
        const key = layoutKey(this)
        const span = key === null ? undefined : LAYOUT[key]
        return span ? span[0] : 0
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
    if (realOffsetLeft) {
      Object.defineProperty(HTMLElement.prototype, "offsetLeft", realOffsetLeft)
    } else {
      delete (HTMLElement.prototype as { offsetLeft?: number }).offsetLeft
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

  function cancel(clientX: number) {
    act(() => {
      fireEvent.pointerCancel(document, {
        pointerId: 1,
        pointerType: "touch",
        clientX,
        clientY: 300,
        isPrimary: true,
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

  // THE HELD STATE, driven rather than asserted from a class name. Both
  // dividers must say "active" in the same attribute for the shared class to
  // mean anything, and both must stop saying it when the finger lifts.
  it("both report themselves held for the length of the gesture", () => {
    for (const mount of [sidebarDivider, changesDivider]) {
      const handle = mount()
      handle.setPointerCapture = () => {}
      expect(handle.getAttribute(DIVIDER_HELD_ATTR)).toBe(DIVIDER_HELD_OFF)
      press(handle, EDGE_CENTRE)
      expect(handle.getAttribute(DIVIDER_HELD_ATTR), mount.name).toBe(
        DIVIDER_HELD_ON,
      )
      move(EDGE_CENTRE + DRAG_BY)
      expect(handle.getAttribute(DIVIDER_HELD_ATTR)).toBe(DIVIDER_HELD_ON)
      release(EDGE_CENTRE + DRAG_BY)
      expect(handle.getAttribute(DIVIDER_HELD_ATTR)).toBe(DIVIDER_HELD_OFF)
      cleanup()
    }
  })

  // THE CANCELLED TOUCH, which is why the held paint is dux's attribute and not
  // the library's. Measured on a touch tablet against react-resizable-panels
  // 4.11.2: it has no `pointercancel` listener, so its separator stays at
  // `data-separator="active"` for good once the browser takes a touch away, and
  // the line stayed lit with no finger anywhere near it. Both dividers must go
  // dark on a cancel, and the library's own attribute is deliberately not
  // asserted here: it is allowed to stay wrong, it just may not be what paints.
  it("both stop looking held when the browser takes the gesture away", () => {
    for (const mount of [sidebarDivider, changesDivider]) {
      const handle = mount()
      handle.setPointerCapture = () => {}
      press(handle, EDGE_CENTRE)
      move(EDGE_CENTRE + DRAG_BY)
      expect(handle.getAttribute(DIVIDER_HELD_ATTR)).toBe(DIVIDER_HELD_ON)
      cancel(EDGE_CENTRE + DRAG_BY)
      expect(handle.getAttribute(DIVIDER_HELD_ATTR), mount.name).toBe(
        DIVIDER_HELD_OFF,
      )
      cleanup()
    }
  })

  // Both hooks focus the divider on pointerdown, and both ask the browser not
  // to treat that as a keyboard arrival. react-resizable-panels 4.11.2 passes
  // `{ focusVisible: false, preventScroll: true }` on its separator; dux's hook
  // passes the same, so a press cannot leave a focus ring on one divider and
  // not the other.
  it("both take focus without asking for a focus ring", () => {
    for (const mount of [sidebarDivider, changesDivider]) {
      const handle = mount()
      handle.setPointerCapture = () => {}
      const focus = vi.fn()
      handle.focus = focus
      press(handle, EDGE_CENTRE)
      expect(focus, mount.name).toHaveBeenCalledWith(
        expect.objectContaining({ focusVisible: false, preventScroll: true }),
      )
      release(EDGE_CENTRE)
      cleanup()
    }
  })

  // A TAP REMEMBERS NOTHING. Measured on a touch tablet: a no-move tap on the
  // sidebar's edge wrote `dux:sidebar-width` (the width it already had, so
  // nothing moved on screen), while the same tap on the Changes divider wrote
  // nothing at all. Storage is the user's record of a width they chose; a tap
  // is not a choice, and writing on one pins whatever the sidebar happened to
  // be at.
  it("the sidebar writes nothing for a press that went nowhere", () => {
    localStorage.removeItem("dux:sidebar-width")
    const handle = sidebarDivider()
    handle.setPointerCapture = () => {}
    press(handle, EDGE_CENTRE)
    release(EDGE_CENTRE)
    expect(localStorage.getItem("dux:sidebar-width")).toBeNull()
  })

  // Jitter is not a drag either: a finger resting on glass wanders a pixel or
  // two, and the shared threshold is what says when it has become a gesture.
  it("the sidebar writes nothing for a press that only jittered", () => {
    localStorage.removeItem("dux:sidebar-width")
    const handle = sidebarDivider()
    handle.setPointerCapture = () => {}
    const jitter = DIVIDER_DRAG_THRESHOLD_PX - 1
    press(handle, EDGE_CENTRE)
    move(EDGE_CENTRE + jitter)
    release(EDGE_CENTRE + jitter)
    expect(localStorage.getItem("dux:sidebar-width")).toBeNull()
  })

  // And a real drag still writes, so the guard above cannot be satisfied by
  // never writing at all.
  it("the sidebar still remembers a drag that travelled", () => {
    expect(dragSidebar()).toBe(288 + DRAG_BY)
  })

  // THE DEAD STRIP. A browser adjusts a touch point before dispatching it:
  // Chrome grows a finger's contact area and picks the most plausible target
  // inside it, which on a tablet reached about 20px either side of the thin
  // sidebar edge. The press then arrived with the edge as its TARGET but with
  // coordinates outside the 10px band the hook was testing, so it was thrown
  // away: the strip between the two widths neither resized nor scrolled.
  //
  // 16px off centre is outside even the coarse band (which reaches 10px each
  // way), so only the browser's own verdict can carry this press.
  it("takes a press the browser has already given to the divider", () => {
    localStorage.removeItem("dux:sidebar-width")
    const handle = sidebarDivider()
    handle.setPointerCapture = () => {}
    const far = 16
    press(handle, EDGE_CENTRE + far)
    move(EDGE_CENTRE + far + DRAG_BY)
    release(EDGE_CENTRE + far + DRAG_BY)
    // Acquired, and still delta-based: the divider moved by the drag, not to
    // the pointer.
    expect(
      sidebarWidthToPx(localStorage.getItem("dux:sidebar-width") ?? "0"),
    ).toBe(288 + DRAG_BY)
  })

  // The band still decides a press the browser gave to a NEIGHBOUR. Without
  // this the target rule would be a licence for anything to grab the divider.
  it("still refuses a press outside the band that landed on something else", () => {
    localStorage.removeItem("dux:sidebar-width")
    const handle = sidebarDivider()
    handle.setPointerCapture = () => {}
    press(document.body, EDGE_CENTRE + 60)
    move(EDGE_CENTRE + 60 + DRAG_BY)
    release(EDGE_CENTRE + 60 + DRAG_BY)
    expect(localStorage.getItem("dux:sidebar-width")).toBeNull()
  })
})
