// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// The startup-command log viewer serves two SCOPES from one dialog: one agent's
// runs, and every run across every agent of a project. What the dialog must get
// right is that a user who opens both in sequence can tell them apart, so the
// title, the subtitle and the empty state are all asserted per scope here.

let mockState: DuxState
const closeStartupLogsSpy = vi.hoisted(() => vi.fn())
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeStartupLogs: closeStartupLogsSpy,
  }
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
const { StartupLogsDialog } = await import("./StartupLogsDialog")

const SPINE = {
  projects: [{ id: "p1", name: "Repo" }],
  sessions: [
    {
      id: "s1",
      title: "Fix login",
      workspace: { kind: "managed", project_id: "p1", branch_name: "fix-login", initial_branch: "fix-login", branch_provenance: "created", source_branch: "main", worktree_path: "/wt/s1" },
    },
  ],
}

function renderOpen(over: Partial<DuxState>) {
  mockState = {
    spine: SPINE,
    startupLogsTarget: null,
    startupLogsScope: "agent",
    startupLogsEntries: [],
    startupLogsSelected: null,
    startupLogsLoading: false,
    startupLogsError: null,
    ...over,
  } as unknown as DuxState
  render(<StartupLogsDialog />)
}

const ONE_RUN = {
  startupLogsEntries: [{ name: "20260101T000000Z-fix-login.log", modified_at: null }],
  startupLogsSelected: {
    name: "20260101T000000Z-fix-login.log",
    content: "install ok",
  },
}

beforeEach(() => {
  installBootStubs()
  closeStartupLogsSpy.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("StartupLogsDialog", () => {
  it("names the AGENT in agent scope", () => {
    renderOpen({ startupLogsTarget: "s1", startupLogsScope: "agent", ...ONE_RUN })
    expect(screen.getByText(/Startup command logs: Fix login/)).toBeTruthy()
    expect(screen.getByText(/in this agent's worktree/)).toBeTruthy()
    expect(screen.getByText("install ok")).toBeTruthy()
  })

  it("renders the log body in the bundled terminal font, not bare system mono", () => {
    renderOpen({ startupLogsTarget: "s1", startupLogsScope: "agent", ...ONE_RUN })
    const body = screen.getByText("install ok")
    expect(body.style.fontFamily).toContain("Dux Mono")
  })

  it("names the PROJECT and says it spans every agent in project scope", () => {
    renderOpen({ startupLogsTarget: "p1", startupLogsScope: "project", ...ONE_RUN })
    expect(screen.getByText(/Startup command logs: Repo \(all agents\)/)).toBeTruthy()
    expect(screen.getByText(/across every agent in this project/)).toBeTruthy()
    // The agent-scope subtitle must not leak into the project view.
    expect(screen.queryByText(/in this agent's worktree/)).toBeNull()
  })

  it("says a project has never run its startup command instead of showing a blank body", () => {
    renderOpen({ startupLogsTarget: "p1", startupLogsScope: "project" })
    expect(
      screen.getByText(/Run the startup command for an agent in this project/),
    ).toBeTruthy()
  })

  it("closes itself when the project it is scoped to vanishes", () => {
    renderOpen({ startupLogsTarget: "gone", startupLogsScope: "project" })
    expect(closeStartupLogsSpy).toHaveBeenCalled()
  })

  it("does not close a project view just because no session matches its id", () => {
    // The vanished-target guard must consult PROJECTS in project scope; keying
    // it off the session list would slam the dialog shut immediately.
    renderOpen({ startupLogsTarget: "p1", startupLogsScope: "project", ...ONE_RUN })
    expect(closeStartupLogsSpy).not.toHaveBeenCalled()
  })
})
