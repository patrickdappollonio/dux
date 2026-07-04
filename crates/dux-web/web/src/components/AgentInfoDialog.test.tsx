// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
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
const { AgentInfoDialog } = await import("./AgentInfoDialog")

const base = {
  id: "s1",
  project_id: "p1",
  provider: "claude",
  worktree_path: "/tmp/s1",
  status: "active",
  created_at: "2026-07-01T00:00:00Z",
  updated_at: "2026-07-02T00:00:00Z",
  terminals: [],
  tabs: [
    {
      id: "s1",
      provider: "claude",
      order: 0,
      working: false,
      has_output: false,
      has_live_process: true,
    },
  ],
}

function renderDialogOpenFor(session: Partial<SessionView>) {
  mockState = {
    agentInfoTarget: "s1",
    spine: {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [{ ...base, ...session } as unknown as SessionView],
    },
  } as unknown as DuxState
  render(<AgentInfoDialog />)
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AgentInfoDialog", () => {
  it("shows current, original, and fork branches and flags drift", () => {
    renderDialogOpenFor({
      title: "server-mode",
      branch_name: "agent-tabs",
      initial_branch: "server-mode",
      source_branch: "main",
    })
    // Current branch.
    expect(screen.getByText("agent-tabs")).toBeTruthy()
    // Original branch (also the dialog title, so there is more than one match).
    expect(screen.getAllByText(/server-mode/).length).toBeGreaterThan(0)
    // Forked from.
    expect(screen.getByText("main")).toBeTruthy()
    // Drift note.
    expect(screen.getByText(/changed since creation/i)).toBeTruthy()
  })

  it("omits the drift note when the current branch matches the original", () => {
    renderDialogOpenFor({
      title: "server-mode",
      branch_name: "server-mode",
      initial_branch: "server-mode",
      source_branch: "main",
    })
    expect(screen.queryByText(/changed since creation/i)).toBeNull()
  })
})
