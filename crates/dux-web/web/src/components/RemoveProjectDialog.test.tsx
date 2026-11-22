// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    removeProject: vi.fn(),
    closeRemoveProject: vi.fn(),
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
}
installBootStubs()
const { RemoveProjectDialog } = await import("./RemoveProjectDialog")
const store = await import("@/lib/store")
const closeRemoveProject = vi.mocked(store.closeRemoveProject)

const ghostAgent = {
  id: "s1",
  workspace: {
    kind: "managed",
    project_id: "ghost",
    branch_name: "",
    initial_branch: "",
    branch_provenance: "created",
    source_branch: "",
    worktree_path: "",
  },
}

function seed(sessions: unknown[], groups: unknown[]) {
  mockState = {
    removeProjectTarget: "ghost",
    spine: { projects: [], sessions, terminals: [], sidebar: { groups } },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  closeRemoveProject.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("RemoveProjectDialog", () => {
  // A ghost project has no record of its own: its sidebar group or its agents
  // are what the dialog is for, so either keeps it open.
  it.each([
    ["agents", [ghostAgent], []],
    ["a sidebar group", [], [{ project_id: "ghost", name: "ghost-1a2b" }]],
  ])("stays open for a ghost project that still has %s", (_, sessions, groups) => {
    seed(sessions, groups)
    render(<RemoveProjectDialog />)
    expect(screen.getByText("Remove project?")).toBeTruthy()
    expect(closeRemoveProject).not.toHaveBeenCalled()
  })

  it("closes itself once neither the project, its group nor its agents are left", () => {
    seed([], [])
    render(<RemoveProjectDialog />)
    expect(screen.queryByText("Remove project?")).toBeNull()
    expect(closeRemoveProject).toHaveBeenCalled()
  })
})
