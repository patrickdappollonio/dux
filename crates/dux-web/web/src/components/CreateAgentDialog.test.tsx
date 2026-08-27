// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { CreateAgentTarget, DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

function installBootStubs() {
  const values = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, String(value)),
    removeItem: (key: string) => values.delete(key),
    clear: () => values.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()

const project = { id: "p1", name: "acme" }
const session = {
  id: "s1",
  title: "quacky-mallard",
  workspace: {
    kind: "managed",
    project_id: "p1",
    branch_name: "dux/s1",
    initial_branch: "dux/s1",
    branch_provenance: "created",
    source_branch: "",
    worktree_path: "",
  },
}

function seed(target: CreateAgentTarget, overrides: Partial<DuxState> = {}) {
  mockState = {
    createAgentTarget: target,
    createAgentDraft: "",
    createAgentRandomize: false,
    createAgentCopyChanges: false,
    createAgentNamePending: false,
    createAgentPrInput: "",
    createAgentPrResolving: false,
    createAgentPrError: null,
    spine: { projects: [project], sessions: [session] },
    ...overrides,
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    callback(0)
    return 0
  })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("CreateAgentDialog", () => {
  it("renders new-agent controls for a project", async () => {
    seed({ kind: "new", projectId: "p1" })
    const { CreateAgentDialog } = await import("./CreateAgentDialog")
    render(<CreateAgentDialog />)

    expect(screen.getByText("New agent in acme")).toBeTruthy()
    expect(screen.getByRole("button", { name: "Create agent" })).toBeTruthy()
    expect(
      screen.getByText("Copy uncommitted changes from the project checkout"),
    ).toBeTruthy()
  })

  it("requires a branch name when forking", async () => {
    seed({ kind: "fork", sessionId: "s1" })
    const { CreateAgentDialog } = await import("./CreateAgentDialog")
    render(<CreateAgentDialog />)

    expect(screen.getByText("Fork quacky-mallard")).toBeTruthy()
    expect(screen.getByPlaceholderText("Branch name")).toBeTruthy()
    expect(
      (screen.getByRole("button", { name: "Fork agent" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true)
  })

  it("renders reference-first PR resolution and its inline error", async () => {
    seed(
      { kind: "pr", projectId: null },
      { createAgentPrError: "That repository is not open." },
    )
    const { CreateAgentDialog } = await import("./CreateAgentDialog")
    render(<CreateAgentDialog />)

    expect(screen.getByText("New agent from PR")).toBeTruthy()
    expect(screen.getByLabelText("GitHub pull request")).toBeTruthy()
    expect(screen.getByRole("alert").textContent).toBe(
      "That repository is not open.",
    )
    expect(screen.getByText("or choose an existing project")).toBeTruthy()
  })
})
