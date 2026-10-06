// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// The project-scoped confirmations name the project, and its base branch where
// there is one, through the shared chip: the chip is what sets the name apart
// from the sentence, so no quotes are left around it.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    deleteProject: vi.fn(),
    closeDeleteProject: vi.fn(),
    removeProject: vi.fn(),
    closeRemoveProject: vi.fn(),
    checkoutDefaultBranch: vi.fn(),
    closeCheckoutDefaultBranch: vi.fn(),
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
const { RemoveProjectDialog } = await import("./RemoveProjectDialog")
const store = await import("@/lib/store")
const { CheckoutDefaultBranchDialog } = await import(
  "./CheckoutDefaultBranchDialog"
)

const project = { id: "p1", name: "duck-pond", leading_branch: "feature/x" }

function seed(over: Partial<DuxState>) {
  mockState = {
    deleteProjectTarget: null,
    removeProjectTarget: null,
    checkoutDefaultBranchTarget: null,
    spine: { projects: [project], sessions: [], sidebar: { groups: [] } },
    ...over,
  } as unknown as DuxState
}

function chips(el: Element): (string | null)[] {
  return [...el.querySelectorAll("code")].map((c) => c.textContent)
}

beforeEach(() => installBootStubs())
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("project confirmations name the project as a chip", () => {
  it("Delete project", () => {
    seed({ deleteProjectTarget: "p1" } as Partial<DuxState>)
    render(<DeleteProjectDialog />)
    const body = screen.getByText(/This deletes/)
    expect(chips(body)).toEqual(["duck-pond"])
    expect(body.textContent).toContain("This deletes duck-pond from dux.")
  })

  it("Remove project", () => {
    seed({ removeProjectTarget: "p1" } as Partial<DuxState>)
    render(<RemoveProjectDialog />)
    const body = screen.getByText(/This removes/)
    expect(chips(body)).toEqual(["duck-pond"])
    expect(body.textContent).toContain("This removes duck-pond from dux.")
  })

  it("Remove project names its agents and focuses Cancel, the safe answer", () => {
    seed({
      removeProjectTarget: "p1",
      spine: {
        projects: [project],
        sessions: [{ id: "s1", workspace: { kind: "managed", project_id: "p1" } }],
        sidebar: { groups: [] },
      },
    } as unknown as Partial<DuxState>)
    render(<RemoveProjectDialog />)
    const body = screen.getByText(/This removes/)
    expect(body.textContent).toBe(
      "This removes duck-pond and deletes its 1 agent from dux. Worktrees on disk are kept.",
    )
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }))
  })

  it("Remove project names who is attached as chips and removes over them only through Remove anyway", async () => {
    seed({
      removeProjectTarget: "p1",
      spine: {
        projects: [project],
        sessions: [],
        terminals: [
          {
            id: "term-3",
            owner: { kind: "project", project_id: "p1" },
            label: "Terminal 3",
          },
        ],
        sidebar: { groups: [] },
      },
    } as unknown as Partial<DuxState>)
    const removeProject = vi.mocked(store.removeProject)
    removeProject.mockResolvedValueOnce([
      {
        surface: "browser",
        device: "Mozilla/5.0 (Linux; Android 14) Chrome/124.0 Mobile Safari/537.36",
        address: "100.64.0.9",
        verified: true,
        driving: false,
        target: { kind: "terminal", id: "term-3" },
      },
    ])
    render(<RemoveProjectDialog />)

    fireEvent.click(screen.getByRole("button", { name: "Remove" }))
    expect(removeProject).toHaveBeenCalledWith("p1", false)
    const override = await screen.findByRole("button", { name: "Remove anyway" })
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }))
    const who = screen.getByText((_, el) => el?.tagName === "LI")
    expect(chips(who)).toEqual(["Chrome on Android", "Terminal 3"])

    removeProject.mockResolvedValueOnce(null)
    fireEvent.click(override)
    expect(removeProject).toHaveBeenLastCalledWith("p1", true)
  })

  it("Check out default branch names the project and its current base", () => {
    seed({ checkoutDefaultBranchTarget: "p1" } as Partial<DuxState>)
    render(<CheckoutDefaultBranchDialog />)
    // The verb form, matching the confirm button and the menu entry.
    expect(screen.getByRole("heading").textContent).toBe(
      "Check out default branch?",
    )
    const body = screen.getByText(/This switches the source checkout/)
    expect(chips(body)).toEqual(["duck-pond", "feature/x"])
    expect(body.textContent).not.toMatch(/["“”]/)
  })
})
