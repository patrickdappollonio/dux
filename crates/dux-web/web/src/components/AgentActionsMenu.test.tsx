// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

// The agent ⋯ menu's pull-request entries. What is pinned here is the GATING:
// the attach item exists only with a usable gh (matching the project menu's
// from-PR gate), its label flips on the OVERRIDE (a manually attached PR), not
// on mere PR presence, and the detach item exists only while an override is
// in place (and needs no gh, since detaching talks to nothing).
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    openAttachPullRequest: vi.fn(),
    detachPullRequest: vi.fn(),
  }
})

// The store touches localStorage at import time (pulled in transitively), so
// stub the browser globals BEFORE the module graph evaluates.
function installStubs() {
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
installStubs()
const { AgentActionsMenu } = await import("./FlatAgentList")
const { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } = await import(
  "@/components/ui/dropdown-menu"
)
const store = await import("@/lib/store")
const openAttachPullRequest = vi.mocked(store.openAttachPullRequest)
const detachPullRequest = vi.mocked(store.detachPullRequest)

function makeSession(over: Partial<SessionView> & { id: string }): SessionView {
  return {
    project_id: "p1",
    title: over.id,
    provider: "claude",
    branch_name: over.id,
    initial_branch: over.id,
    source_branch: "main",
    worktree_path: `/tmp/${over.id}`,
    status: "active",
    auto_reopen_enabled: false,
    tabs: [],
    has_output: false,
    working: false,
    typing: false,
    needs_attention: false,
    created_at: "2026-07-17T12:00:00Z",
    updated_at: "2026-07-17T12:00:00Z",
    ...over,
  } as SessionView
}

function seed(session: SessionView, ghAvailable: boolean) {
  mockState = {
    bootstrap: {
      gh_available: ghAvailable,
      available_providers: ["claude"],
      agent_tabs_max: 20,
    },
    spine: { projects: [], sessions: [session] },
    createTabInFlight: [],
  } as unknown as DuxState
}

function openMenu(session: SessionView) {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <AgentActionsMenu session={session} />
      </DropdownMenuContent>
    </DropdownMenu>,
  )
  fireEvent.click(screen.getByText("open"))
  return screen.findByRole("menu")
}

const prAuto = {
  number: 12,
  state: "open" as const,
  title: "Add a thing",
  url: "https://github.com/o/r/pull/12",
  overridden: false,
}
const prPinned = { ...prAuto, overridden: true }

beforeEach(() => {
  installStubs()
  openAttachPullRequest.mockClear()
  detachPullRequest.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AgentActionsMenu pull-request entries", () => {
  it("hides both entries without a usable gh and no override", async () => {
    seed(makeSession({ id: "s1" }), false)
    await openMenu(makeSession({ id: "s1" }))
    expect(screen.queryByText(/Attach pull request/)).toBeNull()
    expect(screen.queryByText(/attached pull request/)).toBeNull()
    expect(screen.queryByText("Detach pull request")).toBeNull()
  })

  it("offers the attach entry with gh, with icon and trailing ellipsis, routed to the store", async () => {
    const session = makeSession({ id: "s1" })
    seed(session, true)
    await openMenu(session)
    const item = screen
      .getByText("Attach pull request…")
      .closest('[role="menuitem"]')
    expect(item).toBeTruthy()
    // Trailing "…" marks an item that opens a dialog; a leading lucide icon is
    // required on every item in these menus.
    expect(item!.textContent?.endsWith("…")).toBe(true)
    expect(item!.querySelector("svg")).toBeTruthy()
    // No override: no detach entry.
    expect(screen.queryByText("Detach pull request")).toBeNull()
    fireEvent.click(screen.getByText("Attach pull request…"))
    expect(openAttachPullRequest).toHaveBeenCalledWith("s1")
  })

  it("keeps the attach label on an AUTODETECTED PR (label flips on the override, not presence)", async () => {
    const session = makeSession({ id: "s1", pr: prAuto })
    seed(session, true)
    await openMenu(session)
    expect(screen.getByText("Attach pull request…")).toBeTruthy()
    expect(screen.queryByText("Change attached pull request…")).toBeNull()
    expect(screen.queryByText("Detach pull request")).toBeNull()
  })

  it("flips to the change label and offers detach while an override is pinned", async () => {
    const session = makeSession({ id: "s1", pr: prPinned })
    seed(session, true)
    await openMenu(session)
    expect(screen.getByText("Change attached pull request…")).toBeTruthy()
    expect(screen.queryByText("Attach pull request…")).toBeNull()
    const detach = screen
      .getByText("Detach pull request")
      .closest('[role="menuitem"]')
    expect(detach).toBeTruthy()
    // Reversible action: icon yes, but NO trailing ellipsis and no confirm.
    expect(detach!.textContent?.endsWith("…")).toBe(false)
    expect(detach!.querySelector("svg")).toBeTruthy()
    fireEvent.click(screen.getByText("Detach pull request"))
    expect(detachPullRequest).toHaveBeenCalledWith("s1")
  })

  it("keeps detach available without gh (detaching talks to nothing)", async () => {
    const session = makeSession({ id: "s1", pr: prPinned })
    seed(session, false)
    await openMenu(session)
    expect(screen.queryByText(/pull request…/)).toBeNull()
    expect(screen.getByText("Detach pull request")).toBeTruthy()
  })
})
