// @vitest-environment jsdom
//
// The workspace has two draggable dividers and they must feel the same. The
// left one is dux's own (the sidebar's width is a CSS variable, not a panel in
// a layout group); the right one is react-resizable-panels'. They drifted once,
// and a finger could move one and not the other. This file is what makes that
// drift a failing test rather than a bug report from a tablet.
import { describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { afterEach } from "vitest"

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

const { SidebarProvider } = await import("@/components/ui/sidebar")
const { AppSidebar } = await import("./Sidebar")
const { ResizableHandle, ResizablePanel, ResizablePanelGroup } = await import(
  "@/components/ui/resizable"
)
const { DIVIDER_CHROME, DIVIDER_TARGET_MIN } = await import(
  "@/lib/paneDivider"
)

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
