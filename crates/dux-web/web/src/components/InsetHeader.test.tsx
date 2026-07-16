// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// The store boots on import and reaches for browser globals; stub them so the
// render stays hermetic and off the network (mirrors the sibling tests).
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
const { InsetHeader } = await import("./InsetHeader")

function stateFor(branchName: string, initialBranch: string): DuxState {
  return {
    selectedSessionId: "s1",
    selectedTarget: { kind: "agent", sessionId: "s1", tabId: "s1" },
    spine: {
      projects: [{ id: "p1", name: "Repo" }],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: null,
          provider: "claude",
          branch_name: branchName,
          initial_branch: initialBranch,
          source_branch: "main",
          worktree_path: "/tmp/s1",
          status: "active",
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
        },
      ],
    },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("InsetHeader app menu", () => {
  it("renders the app-menu cog instead of a Commands button", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.queryByText(/Commands/)).toBeNull()
    expect(screen.getByRole("button", { name: /^menu$/i })).toBeTruthy()
  })
})

describe("InsetHeader project terminal crumbs", () => {
  it("renders project and terminal crumbs for a focused project terminal", () => {
    // The trap this guards (T8): every crumb was gated on a resolved SESSION,
    // so a focused project terminal rendered a completely blank breadcrumb bar.
    mockState = {
      selectedSessionId: null,
      selectedTarget: {
        kind: "terminal",
        terminalId: "pt-1",
        owner: { kind: "project", projectId: "p1" },
      },
      spine: {
        projects: [
          {
            id: "p1",
            name: "Repo",
            terminals: [
              {
                id: "pt-1",
                label: "Terminal 2",
                has_output: true,
                foreground_cmd: null,
              },
            ],
          },
        ],
        sessions: [],
      },
    } as unknown as DuxState
    render(<InsetHeader />)
    expect(screen.getByText("Repo")).toBeTruthy()
    expect(screen.getByText("Terminal 2")).toBeTruthy()
    // The crumb keys name the owner kind.
    expect(screen.getByText(/project:/)).toBeTruthy()
    expect(screen.getByText(/terminal:/)).toBeTruthy()
  })
})

describe("InsetHeader branch drift cue", () => {
  it("shows the original branch only when the current branch differs", () => {
    mockState = stateFor("agent-tabs", "server-mode")
    render(<InsetHeader />)
    expect(screen.getByText(/originally server-mode/)).toBeTruthy()
  })

  it("omits the original branch when it matches the current branch", () => {
    mockState = stateFor("main", "main")
    render(<InsetHeader />)
    expect(screen.queryByText(/originally/)).toBeNull()
  })
})
