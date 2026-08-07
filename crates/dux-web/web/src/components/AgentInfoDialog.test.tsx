// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

let mockState: DuxState
const closeAgentInfoSpy = vi.hoisted(() => vi.fn())
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, closeAgentInfo: closeAgentInfoSpy }
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
  closeAgentInfoSpy.mockClear()
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

  it("shows the pull request row, naming a manual pin", () => {
    // Pinned: the row carries the "manually attached" cue, which is where a
    // pin says it is one (matching the TUI's Agent Info line).
    renderDialogOpenFor({
      title: "server-mode",
      branch_name: "feat",
      initial_branch: "feat",
      source_branch: "main",
      pr: {
        number: 12,
        state: "open",
        title: "Fix the frobnicator",
        url: "https://github.com/o/r/pull/12",
        overridden: true,
      },
    })
    expect(screen.getByText(/#12 \(open\) Fix the frobnicator/)).toBeTruthy()
    expect(screen.getByText(/manually attached/)).toBeTruthy()
  })

  it("shows a detected pull request without the manual cue", () => {
    renderDialogOpenFor({
      title: "server-mode",
      branch_name: "feat",
      initial_branch: "feat",
      source_branch: "main",
      pr: {
        number: 12,
        state: "merged",
        title: "Fix the frobnicator",
        url: "https://github.com/o/r/pull/12",
        overridden: false,
      },
    })
    expect(screen.getByText(/#12 \(merged\) Fix the frobnicator/)).toBeTruthy()
    expect(screen.queryByText(/manually attached/)).toBeNull()
  })

  it("omits the pull request row when the session has none", () => {
    renderDialogOpenFor({
      title: "server-mode",
      branch_name: "feat",
      initial_branch: "feat",
      source_branch: "main",
    })
    expect(screen.queryByText(/Pull request/)).toBeNull()
  })

  it("closes when the target session is no longer in the spine", () => {
    // The dialog's target points at an id absent from `spine.sessions` (the agent
    // was removed while the dialog was open). The vanished-target effect fires
    // `closeAgentInfo` so the modal doesn't linger pointing at a gone agent.
    mockState = {
      agentInfoTarget: "gone",
      spine: {
        projects: [{ id: "p1", name: "Repo" }],
        sessions: [{ ...base } as unknown as SessionView],
      },
    } as unknown as DuxState
    render(<AgentInfoDialog />)
    expect(closeAgentInfoSpy).toHaveBeenCalled()
  })

  it("shows the Unknown fallback and no drift note for a legacy session", () => {
    // An older server omits the branch fields (coerced to "" at ingestion).
    renderDialogOpenFor({
      title: "server-mode",
      branch_name: "server-mode",
      initial_branch: "",
      source_branch: "",
    })
    // Both the Original branch and Forked from rows fall back to "Unknown".
    expect(screen.getAllByText("Unknown").length).toBe(2)
    // No drift note when the original branch is absent.
    expect(screen.queryByText(/changed since creation/i)).toBeNull()
  })
})
