// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

// EVERY SHELL HAS A WAY BACK, and this is the test that says so across all of
// them at once.
//
// It replaces the old "the input `⋯` renders in every bar state" contract.
// The bottom `⋯` lives on whatever row is under the terminal, and a pane can
// have no rows at all, so the promise moved: whatever the pane is doing, SOME
// menu carries "Use virtual input". ONE HOME AT A TIME, and the pane decides
// which: while any bottom row is up it is the bottom `⋯`, and only once
// nothing is left below does the top menu carry the way back. The per-surface
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

const { PaneMenuBody } = await import("./PaneMenu")
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
    // The ids are deliberately all different from one another here: a session
    // id, a tab id and a terminal id come from different counters, and a
    // fixture that reuses one for another makes a menu reading the wrong id
    // space pass anyway.
    id: "s1",
    slot_tab_id: "tab1",
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
    tabs: [{ id: "tab1", provider: "claude", order: 0 }],
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

// Every top-menu body a pane can be reached through, in the plainest anchor
// that can open one. The bodies are what the shells render; opening them here
// rather than driving each shell keeps this about the contract and not about
// layout.
//
// An agent's four anchors (the phone's flap and pill, the desktop pane
// header's `⋯` and the sidebar row's) are ONE body, so listing them separately
// would test the same component twice. What has to be listed separately is
// each SUBJECT, because a terminal's menu is a different body with the same
// promise to keep.
//
// EACH SHELL SAYS WHICH PTY ID ITS PANE PUBLISHES UNDER, because that is the
// whole question the group is read by.
const SHELLS = {
  agent: {
    ptyId: "tab1",
    body: () => (
      <PaneMenuBody
        subject={{ kind: "agent", session: session() }}
        pane={{ kind: "agent", sessionId: "s1", tabId: "tab1" }}
        settingsDrill={false}
      />
    ),
  },
  terminal: {
    ptyId: "t9",
    body: () => (
      <PaneMenuBody
        subject={{
          kind: "terminal",
          terminalId: "t9",
          owner: { kind: "standalone" },
        }}
        pane={{
          kind: "terminal",
          terminalId: "t9",
          owner: { kind: "standalone" },
        }}
        settingsDrill={false}
      />
    ),
  },
  // THE CASE THAT WAS BROKEN: a companion terminal's pane wears its AGENT's
  // menu, and that pane publishes under the TERMINAL's id. Reading the group
  // off the subject looked among the agent's ids and found nothing, so this
  // pane had no "Attach a file…" and, once the user had asked to type straight
  // into the terminal, no way back at all.
  "agent-owned terminal": {
    ptyId: "t7",
    body: () => (
      <PaneMenuBody
        subject={{ kind: "agent", session: session() }}
        pane={{
          kind: "terminal",
          terminalId: "t7",
          owner: { kind: "session", sessionId: "s1" },
        }}
        settingsDrill={false}
      />
    ),
  },
  // A SIDEBAR ROW is not painted over a pane, so it scans the subject's own
  // ptys: the session-slot id and every tab id, any one of which can be the
  // mounted pane.
  "sidebar row": {
    ptyId: "tab1",
    body: () => (
      <PaneMenuBody
        subject={{ kind: "agent", session: session() }}
        settingsDrill={false}
      />
    ),
  },
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
  for (const [shell, { ptyId, body }] of Object.entries(SHELLS)) {
    it(`is in a top menu for an ${shell} pane`, async () => {
      // What a pane publishes once the bottom bar is gone: the way back is
      // the top menu's to offer, because nothing else is on screen to offer it.
      registerPaneInputGroup(ptyId, { surfaceSwitch: true, keysToggle: false })
      await openBody(body())
      expect(screen.getByText(WAY_BACK)).toBeTruthy()
    })

    it(`leaves it out for an ${shell} pane while a bottom row is up`, async () => {
      // Absent, never disabled: the bottom `⋯` owns both directions while any
      // row it can hang off exists, and one row must never be in two menus at
      // once. That covers the key row standing alone with the message box
      // gone, which is what the pane publishes here.
      registerPaneInputGroup(ptyId, { surfaceSwitch: false, keysToggle: false })
      await openBody(body())
      expect(screen.queryByText(WAY_BACK)).toBeNull()
    })
  }

  // The other half of the pane-driven rule: a menu must not pick up a SIBLING
  // pane's group. An agent's other tab publishing a way back says nothing
  // about the companion terminal the user is actually looking at.
  it("ignores a sibling pane's group for an agent-owned terminal", async () => {
    registerPaneInputGroup("tab1", { surfaceSwitch: true, keysToggle: false })
    await openBody(SHELLS["agent-owned terminal"].body())
    expect(screen.queryByText(WAY_BACK)).toBeNull()
  })
})
