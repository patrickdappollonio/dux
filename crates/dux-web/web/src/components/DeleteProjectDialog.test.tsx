// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override `useDux` (seeded spine + target) and spy the store actions the
// dialog fires, while every other store export stays intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    deleteProject: vi.fn(),
    closeDeleteProject: vi.fn(),
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
const { DeleteProjectDialog } = await import("./DeleteProjectDialog")
const store = await import("@/lib/store")
const deleteProject = vi.mocked(store.deleteProject)
const closeDeleteProject = vi.mocked(store.closeDeleteProject)

const project1 = { id: "p1", name: "duck-pond" }
const sessionA = {
  id: "s1",
  workspace: {
    kind: "managed",
    project_id: "p1",
    branch_name: "",
    initial_branch: "",
    branch_provenance: "created",
    source_branch: "",
    worktree_path: "",
  },
}
const sessionB = {
  id: "s2",
  workspace: {
    kind: "managed",
    project_id: "p1",
    branch_name: "",
    initial_branch: "",
    branch_provenance: "created",
    source_branch: "",
    worktree_path: "",
  },
}

function seed(target: string | null, projects: unknown[], sessions: unknown[]) {
  mockState = {
    deleteProjectTarget: target,
    spine: { projects, sessions, terminals: [] },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  deleteProject.mockClear()
  closeDeleteProject.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("DeleteProjectDialog", () => {
  it("opens for an existing project and spells out the cascade (agents + worktrees)", () => {
    seed("p1", [project1], [sessionA, sessionB])
    render(<DeleteProjectDialog />)
    expect(screen.getByText("Delete project?")).toBeTruthy()
    expect(screen.getByText(/duck-pond/)).toBeTruthy()
    // The two agents and their worktrees must be named so the user knows what
    // the cascade removes.
    expect(screen.getByText(/2 agents/)).toBeTruthy()
    expect(screen.getByText(/worktrees on disk/)).toBeTruthy()
  })

  // Refused while somebody else is attached to one of its terminals, it stays
  // open naming them and deletes only through Delete anyway.
  it("dispatches the cascade delete on confirm, over who is attached only once they are named", async () => {
    seed("p1", [project1], [sessionA])
    deleteProject.mockResolvedValueOnce([
      {
        surface: "terminal_ui",
        device: "the dux TUI",
        address: null,
        verified: true,
        driving: true,
        target: { kind: "terminal", id: "term-9" },
        key: "k1",
      },
    ])
    render(<DeleteProjectDialog />)
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(deleteProject).toHaveBeenCalledWith("p1", null)
    const override = await screen.findByRole("button", { name: "Delete anyway" })
    expect(screen.getByText("the dux TUI").tagName).toBe("CODE")
    expect(closeDeleteProject).not.toHaveBeenCalled()

    deleteProject.mockResolvedValueOnce(null)
    fireEvent.click(override)
    expect(deleteProject).toHaveBeenLastCalledWith("p1", ["k1"])
    await vi.waitFor(() => expect(closeDeleteProject).toHaveBeenCalled())
  })

  it("closes itself when the target project vanishes mid-open", () => {
    seed("p1", [project1], [sessionA])
    const { rerender } = render(<DeleteProjectDialog />)
    // The project disappears from the live ViewModel (removed elsewhere).
    seed("p1", [], [])
    rerender(<DeleteProjectDialog />)
    expect(closeDeleteProject).toHaveBeenCalled()
  })
})
