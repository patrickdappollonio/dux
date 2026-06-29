// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override only `useDux` so the mobile drawer header's wiring — `bootstrap.title`
// → `resolveInstanceTitle` → the rendered wordmark — is exercised end to end,
// keeping the version/subtitle line intact below it.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// jsdom lacks localStorage/fetch/matchMedia as globals; the real store boots on
// import. Stub them before the component (and the store behind it) loads so the
// render tests are hermetic and off the network.
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
const { MobileShell } = await import("./MobileShell")

function makeState(overrides: Partial<DuxState> = {}): DuxState {
  return {
    spine: null,
    bootstrap: { title: "dux #1", dux_version: "v9.9.9" },
    selectedTarget: null,
    pendingSessionOrder: null,
    pendingProjectOrder: null,
    auth: { phase: "disabled" },
    mobileScreen: "home",
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

describe("MobileShell drawer header", () => {
  it("renders the configured instance title above the 'agent sessions' subtitle", () => {
    mockState = makeState()
    render(<MobileShell />)
    expect(screen.getByText("dux #1")).toBeTruthy()
    // The subtitle is unchanged, proving the title replaced only the wordmark.
    expect(screen.getByText("agent sessions")).toBeTruthy()
  })

  it("falls back to 'dux' when no title is configured", () => {
    mockState = makeState({
      bootstrap: { dux_version: "v9.9.9" } as DuxState["bootstrap"],
    })
    render(<MobileShell />)
    expect(screen.getByText("dux")).toBeTruthy()
  })
})
