// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

// EVERY SHELL HAS A WAY BACK, and this is the test that says so across all of
// them at once.
//
// It replaces the old "the input `⋯` renders in every bar state" contract.
// Choosing to type directly in the terminal now takes the whole bottom bar
// away, that `⋯` with it, so the promise moved: whatever the pane is doing,
// SOME menu the surface always has carries "Use virtual input". The per-surface
// files pin each menu's own behavior; what this pins is that no shell was left
// out of the move, which is exactly the failure a per-file suite cannot see.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

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
installStubs()

const { MobilePaneMenuBody } = await import("./MobilePaneMenu")
const { AgentActionsMenu } = await import("./FlatAgentList")
const {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} = await import("./ui/dropdown-menu")
const { registerPaneInputGroup, resetPaneInputGroups } = await import(
  "@/lib/paneInputGroup"
)

const WAY_BACK = "Use virtual input"

function session(): SessionView {
  return {
    id: "s1",
    slot_tab_id: "s1",
    provider: "claude",
    title: "s1",
    status: "active",
    auto_reopen_enabled: false,
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "b",
      initial_branch: "b",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/w",
    },
    tabs: [{ id: "s1", provider: "claude", order: 0 }],
  } as unknown as SessionView
}

function makeState(): DuxState {
  return {
    spine: { projects: [{ id: "p1", name: "dux" }], sessions: [session()] },
    bootstrap: {
      title: "dux",
      agent_tabs_max: 4,
      available_providers: ["claude"],
      gh_available: false,
    },
    theater: false,
    createTabInFlight: [],
    changes: { sessionId: "s1", phase: "loaded", staged: [], unstaged: [] },
  } as unknown as DuxState
}

// The two shells' top-menu bodies, each in the plainest anchor that can open
// one. The bodies are what the shells render; opening them here rather than
// driving each shell keeps this about the contract and not about layout.
const SHELLS = {
  // A phone: the one pane menu, opened from the flap or from the pill.
  phone: () => <MobilePaneMenuBody session={session()} />,
  // A computer: the pane's own row menu in the sidebar.
  computer: () => <AgentActionsMenu session={session()} />,
} as const

async function openBody(body: React.ReactNode) {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>{body}</DropdownMenuContent>
    </DropdownMenu>,
  )
  fireEvent.click(screen.getByText("open"))
  await screen.findByRole("menu")
}

beforeEach(() => {
  installStubs()
  mockState = makeState()
  resetPaneInputGroups()
})

afterEach(() => {
  cleanup()
  resetPaneInputGroups()
  vi.unstubAllGlobals()
})

describe("the way back from typing directly in the terminal", () => {
  for (const [shell, body] of Object.entries(SHELLS)) {
    it(`is in a top menu on the ${shell}`, async () => {
      // What a pane publishes once the bottom bar is gone: the way back is
      // the top menu's to offer, because nothing else is on screen to offer it.
      registerPaneInputGroup("s1", { surfaceSwitch: true, keysToggle: false })
      await openBody(body())
      expect(screen.getByText(WAY_BACK)).toBeTruthy()
    })

    it(`leaves it out on the ${shell} while the virtual input is up`, async () => {
      // Absent, never disabled: the bottom `⋯` owns the other direction while
      // it exists, and one row must never be in two menus at once.
      registerPaneInputGroup("s1", { surfaceSwitch: false, keysToggle: false })
      await openBody(body())
      expect(screen.queryByText(WAY_BACK)).toBeNull()
    })
  }
})
