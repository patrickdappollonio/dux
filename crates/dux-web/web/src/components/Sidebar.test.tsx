// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { SidebarProvider } from "@/components/ui/sidebar"
import type { DuxState } from "@/lib/store"

// Control exactly what the store hands the component: override only `useDux`
// (keeping every other real store export intact) so the brand-block wiring
// (`bootstrap.title` to `resolveInstanceTitle` to the rendered wordmark) is
// exercised end to end. This guards against a regression that silently swaps the
// title for another field (e.g. the version) or re-hardcodes "dux".
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// The real store module boots on import (it reads localStorage and fires the
// bootstrap fetch + reconnect timers). jsdom doesn't expose localStorage/fetch as bare
// globals, so stub them BEFORE the component (and the store behind it) loads, and
// keep the boot off the network so these render tests stay hermetic.
function installBootStubs() {
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
  // jsdom does not implement matchMedia, which the sidebar's responsive hook uses.
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
}
installBootStubs()
const { AppSidebar } = await import("./Sidebar")

function makeState(overrides: Partial<DuxState> = {}): DuxState {
  return {
    spine: null,
    bootstrap: { title: "dux #1", dux_version: "v9.9.9" },
    selectedTarget: null,
    pendingSessionOrder: null,
    pendingProjectOrder: null,
    ...overrides,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AppSidebar brand block", () => {
  it("renders the configured instance title with the version below it", () => {
    mockState = makeState()
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    // The configured title is the wordmark; the version is a separate line. Both
    // present and distinct proves the two fields were not swapped.
    expect(screen.getByText("dux #1")).toBeTruthy()
    expect(screen.getByText("v9.9.9")).toBeTruthy()
  })

  it("falls back to 'dux' when no title is configured", () => {
    mockState = makeState({
      bootstrap: { dux_version: "v9.9.9" } as DuxState["bootstrap"],
    })
    render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )
    expect(screen.getByText("dux")).toBeTruthy()
  })
})

describe("AppSidebar resize affordances", () => {
  // The agents panel resizes by dragging only — matching the changes panel. The
  // old shadcn `SidebarRail` doubled as a click-near-the-edge collapse target; it
  // was removed so a stray click by the splitter can no longer collapse the panel.
  // Collapse now happens only through the footer button or the Ctrl/Cmd-B shortcut
  // (the latter lives in SidebarProvider), and the edge offers drag-to-resize when
  // expanded and click-to-expand when collapsed.
  it("exposes the drag handle but not the click-to-collapse rail", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    // No click-to-collapse rail near the splitter.
    expect(container.querySelector('[data-sidebar="rail"]')).toBeNull()

    // Drag-to-resize handle is present.
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeTruthy()

    // The dedicated collapse button in the footer stays.
    expect(
      container.querySelector('[data-sidebar="trigger"]'),
    ).toBeTruthy()
  })

  it("dragging the handle resizes and persists the panel width", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = container.querySelector(
      '[data-sidebar="resize-handle"]',
    ) as HTMLElement
    expect(handle).toBeTruthy()
    // jsdom doesn't implement pointer capture; the handler calls it on press.
    handle.setPointerCapture = () => {}

    // Press, drag to x=400, release. 400px is inside [224, 448] (14rem..28rem),
    // so it lands at exactly 25rem and is persisted on release. This exercises
    // the real window-listener drag path, not just the element's presence.
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 240 })
    window.dispatchEvent(new MouseEvent("pointermove", { clientX: 400 }))
    window.dispatchEvent(new MouseEvent("pointerup", { clientX: 400 }))

    expect(localStorage.getItem("dux:sidebar-width")).toBe("25rem")
  })

  it("clamps the dragged width to the maximum", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider>
        <AppSidebar />
      </SidebarProvider>,
    )

    const handle = container.querySelector(
      '[data-sidebar="resize-handle"]',
    ) as HTMLElement
    handle.setPointerCapture = () => {}

    // 9999px is well past the 28rem (448px) cap, so it must clamp to 28rem.
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 240 })
    window.dispatchEvent(new MouseEvent("pointerup", { clientX: 9999 }))

    expect(localStorage.getItem("dux:sidebar-width")).toBe("28rem")
  })

  it("offers a click-to-expand edge when collapsed, never a collapse target", () => {
    mockState = makeState()
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <AppSidebar />
      </SidebarProvider>,
    )

    // Collapsed: the drag handle is gone, replaced by an expand-only strip; the
    // old collapse rail is still absent.
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeNull()
    expect(container.querySelector('[data-sidebar="rail"]')).toBeNull()
    const expand = container.querySelector(
      '[data-sidebar="expand-handle"]',
    ) as HTMLElement
    expect(expand).toBeTruthy()

    // Clicking the strip expands the panel: the drag handle returns and the
    // expand strip is replaced. (If the strip could collapse, this would loop.)
    fireEvent.click(expand)
    expect(
      container.querySelector('[data-sidebar="resize-handle"]'),
    ).toBeTruthy()
    expect(
      container.querySelector('[data-sidebar="expand-handle"]'),
    ).toBeNull()
  })
})
