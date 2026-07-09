// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// Override `useDux` (seeded spine + target) and spy `closeAgentEnv`, while
// every other store export stays intact.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeAgentEnv: vi.fn(),
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
const { AgentEnvDialog } = await import("./AgentEnvDialog")
const store = await import("@/lib/store")
const closeAgentEnv = vi.mocked(store.closeAgentEnv)

const session = { id: "s1", title: "quacky-mallard", branch_name: "dux/s1", project_id: "p1" }
const project = { id: "p1", name: "acme", env: {} }

function seed(target: string | null, sessions: unknown[], projects: unknown[]) {
  mockState = {
    agentEnvTarget: target,
    spine: { sessions, projects },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  closeAgentEnv.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("AgentEnvDialog", () => {
  it("renders the form when session and project resolve", () => {
    seed("s1", [session], [project])
    render(<AgentEnvDialog />)
    expect(screen.getByText("Environment — quacky-mallard")).toBeTruthy()
    expect(closeAgentEnv).not.toHaveBeenCalled()
  })

  it("closes when the session is missing", () => {
    seed("s1", [], [project])
    render(<AgentEnvDialog />)
    expect(screen.queryByText(/Environment —/)).toBeNull()
    expect(closeAgentEnv).toHaveBeenCalled()
  })

  it("closes when the session exists but its project is missing", () => {
    seed("s1", [session], [])
    render(<AgentEnvDialog />)
    expect(screen.queryByText(/Environment —/)).toBeNull()
    expect(closeAgentEnv).toHaveBeenCalled()
  })
})
