// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeProjectInfo: vi.fn(),
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
const { ProjectInfoDialog } = await import("./ProjectInfoDialog")

function project(leading_branch: string | null) {
  return {
    id: "p1",
    name: "duck-pond",
    path: "/code/duck-pond",
    default_provider: "claude",
    explicit_default_provider: null,
    auto_reopen_agents: null,
    startup_command: null,
    env: {},
    current_branch: "main",
    branch_status: "leading",
    path_missing: false,
    leading_branch,
    created_at: "",
  }
}

function seed(leading: string | null) {
  mockState = {
    projectInfoTarget: "p1",
    spine: { projects: [project(leading)], sessions: [], terminals: [] },
  } as unknown as DuxState
}

afterEach(() => {
  cleanup()
})

describe("ProjectInfoDialog", () => {
  // The branch new agents start from is the project's BASE branch, the name
  // the Change base branch action and the terminal UI's Project info use.
  it("labels the recorded base branch as the base branch", () => {
    seed("develop")
    render(<ProjectInfoDialog />)
    expect(screen.getByText("Base branch")).toBeTruthy()
    expect(screen.queryByText("Default branch")).toBeNull()
    expect(screen.getByText("develop")).toBeTruthy()
  })

  it("says so when no base is recorded yet", () => {
    seed(null)
    render(<ProjectInfoDialog />)
    expect(screen.getByText("Base branch")).toBeTruthy()
    expect(screen.getByText("No base recorded yet")).toBeTruthy()
  })
})
