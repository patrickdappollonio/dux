// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { AgentTabView } from "@/lib/types"

// Override `useDux` so the dialog reads our seeded spine + close target, and
// spy `closeCloseTab` so the vanished-target guard is observable; the other
// real store exports stay intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, closeCloseTab: vi.fn() }
})

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
}
installBootStubs()
const { ConfirmCloseTabDialog } = await import("./ConfirmCloseTabDialog")
const store = await import("@/lib/store")
const closeCloseTab = vi.mocked(store.closeCloseTab)

function tab(overrides: Partial<AgentTabView>): AgentTabView {
  return {
    id: "s1",
    provider: "claude",
    order: 0,
    working: false,
    has_output: false,
    has_live_process: true,
    ...overrides,
  }
}

// Seed the dialog to close `tabId` of a session whose tabs are `tabs`.
function seed(tabId: string, tabs: AgentTabView[]) {
  mockState = {
    closeTabTarget: { sessionId: "s1", tabId },
    spine: {
      sessions: [{ id: "s1", tabs }],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  closeCloseTab.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmCloseTabDialog", () => {
  it("warns the agent detaches when closing its last LIVE tab (a dormant sibling doesn't count)", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/last live tab, so the agent detaches/)).toBeTruthy()
    // The provider name is named in the body.
    expect(screen.getByText(/ends the claude session/)).toBeTruthy()
  })

  // The way back from a closed tab is a NEW tab, and a new tab always starts
  // fresh, so the copy must point at the provider's own history command rather
  // than at a resume dux will not perform.
  it("says a new tab starts fresh and names the history command", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: true }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/deletes the tab for good/)).toBeTruthy()
    expect(screen.getByText(/A new tab always starts fresh/)).toBeTruthy()
    expect(screen.getByText(/history command/)).toBeTruthy()
  })

  it("shows no detach warning when a live sibling keeps the agent running", () => {
    seed("s1", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: true }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(screen.queryByText(/the agent detaches/)).toBeNull()
  })

  // The closed tab itself is DORMANT (has_live_process: false, liveTabs counts 0
  // among OTHER tabs): the `liveTabs === 0` branch of `willDetach`. Closing an
  // already-dormant tab is still meaningful: it deletes the
  // dormant tab's row (or, for the session-slot tab, its slot) outright.
  it("shows no detach warning when closing an already-dormant tab that has a live sibling", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude", has_live_process: true }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(screen.queryByText(/the agent detaches/)).toBeNull()
  })

  it("warns the agent detaches when closing an already-dormant tab with no live sibling", () => {
    seed("b2", [
      tab({ id: "s1", provider: "claude", has_live_process: false }),
      tab({ id: "b2", provider: "codex", has_live_process: false }),
    ])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText(/last live tab, so the agent detaches/)).toBeTruthy()
  })

  // The vanished-target guard: the tab (or its session) disappearing from the
  // ViewModel while the dialog is open must close it instead of leaving a
  // stale confirm pointed at a gone target.
  it("closes itself when the target tab is gone from the ViewModel", () => {
    seed("gone-tab", [tab({ id: "s1", provider: "claude" })])
    render(<ConfirmCloseTabDialog />)
    expect(screen.queryByText("Close tab?")).toBeNull()
    expect(closeCloseTab).toHaveBeenCalled()
  })

  it("stays open (and does not self-close) while the target tab still exists", () => {
    seed("s1", [tab({ id: "s1", provider: "claude" })])
    render(<ConfirmCloseTabDialog />)
    expect(screen.getByText("Close tab?")).toBeTruthy()
    expect(closeCloseTab).not.toHaveBeenCalled()
  })

  it("links to the docs, safely, in a new tab", () => {
    seed("s1", [tab({ id: "s1", provider: "claude" })])
    render(<ConfirmCloseTabDialog />)
    const link = screen.getByRole("link", { name: /how closing a tab works/i })
    expect(link.getAttribute("href")).toBe(
      "https://getdux.app/docs/agent-tabs#closing-a-tab-is-one-way",
    )
    expect(link.getAttribute("target")).toBe("_blank")
    // noopener/noreferrer: don't hand the docs tab a window.opener back into dux.
    expect(link.getAttribute("rel")).toContain("noopener")
    expect(link.getAttribute("rel")).toContain("noreferrer")
  })
})
