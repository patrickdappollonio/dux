// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react"

import type { ChangesSlice, DuxState } from "@/lib/store"

// The Changes pane's "Refresh changes" item has exactly one job that a reader
// cannot see by looking at it: it must take the FORCING path. The store's
// `refreshChanges` only re-GETs, and the server answers that from the very cache
// this action exists to bypass, so an item wired to it would look like it worked
// and change nothing. Two comments in the source warn about that, and a comment
// cannot fail a build, so this mounts the component and clicks the real item.

const forceRefreshChanges = vi.fn(() => Promise.resolve())
const refreshChanges = vi.fn()

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    forceRefreshChanges: () => forceRefreshChanges(),
    refreshChanges: () => refreshChanges(),
  }
})

// The real store boots at import time and touches localStorage and fetch, and
// the pane renders a base-ui ScrollArea whose viewport probes APIs jsdom does
// not implement. `matches: false` plus jsdom's 1024px width put this on the
// desktop layout.
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
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  if (!Element.prototype.getAnimations) {
    Element.prototype.getAnimations = () => []
  }
}

installBootStubs()
const { ChangedFiles } = await import("./ChangedFiles")

function loadedChanges(): ChangesSlice {
  return {
    sessionId: "s1",
    phase: "loaded",
    rev: 1,
    staged: [],
    unstaged: [],
    error: null,
  }
}

async function openActionsMenu() {
  render(<ChangedFiles />)
  fireEvent.click(screen.getByLabelText("Changes actions"))
  return within(await screen.findByRole("menu"))
}

beforeEach(() => {
  installBootStubs()
  forceRefreshChanges.mockClear()
  refreshChanges.mockClear()
  mockState = {
    selectedSessionId: "s1",
    changes: loadedChanges(),
  } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("the Changes pane's Refresh changes action", () => {
  it("forces the server to ask git again rather than re-reading its cache", async () => {
    const menu = await openActionsMenu()

    fireEvent.click(menu.getByText("Refresh changes"))

    expect(forceRefreshChanges).toHaveBeenCalledTimes(1)
    expect(refreshChanges).not.toHaveBeenCalled()
  })

  it("keeps the leading icon the menu conventions require, and no ellipsis", async () => {
    const menu = await openActionsMenu()

    const item = menu.getByText("Refresh changes").closest('[role="menuitem"]')
    expect(item).toBeTruthy()
    expect(item!.querySelector("svg")).toBeTruthy()
    // A trailing "…" marks an item that opens a dialog or needs confirming.
    // This one does neither.
    expect(item!.textContent?.endsWith("…")).toBe(false)
  })
})
