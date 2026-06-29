// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { SidebarProvider } from "@/components/ui/sidebar"
import type { DuxState } from "@/lib/store"

// Control exactly what the store hands the component: override only `useDux`
// (keeping every other real store export intact) so the brand-block wiring —
// `bootstrap.title` → `resolveInstanceTitle` → the rendered wordmark — is
// exercised end to end. This guards against a regression that silently swaps the
// title for another field (e.g. the version) or re-hardcodes "dux".
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// The real store module boots on import (it reads localStorage and fires an auth
// probe + reconnect timers). jsdom doesn't expose localStorage/fetch as bare
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
