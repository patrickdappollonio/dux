// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

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

let phone = false
vi.mock("@/hooks/use-mobile", () => ({
  MOBILE_BREAKPOINT: 768,
  isMobileViewport: () => phone,
  useIsMobile: () => phone,
}))

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
  phone = false
  vi.unstubAllGlobals()
})

describe("AgentEnvDialog", () => {
  it("renders the form when session and project resolve", () => {
    seed("s1", [session], [project])
    render(<AgentEnvDialog />)
    // The agent in the title and the project in the body are both chips.
    expect(screen.getByRole("heading").textContent).toBe(
      "Environment: quacky-mallard",
    )
    expect(screen.getByText("quacky-mallard", { selector: "code" })).toBeTruthy()
    expect(screen.getByText("acme", { selector: "code" })).toBeTruthy()
    expect(closeAgentEnv).not.toHaveBeenCalled()
  })

  it("closes when the session is missing", () => {
    seed("s1", [], [project])
    render(<AgentEnvDialog />)
    expect(screen.queryByText(/Environment:/)).toBeNull()
    expect(closeAgentEnv).toHaveBeenCalled()
  })

  it("closes when the session exists but its project is missing", () => {
    seed("s1", [session], [])
    render(<AgentEnvDialog />)
    expect(screen.queryByText(/Environment:/)).toBeNull()
    expect(closeAgentEnv).toHaveBeenCalled()
  })

  it("keeps the same box, with its focus and what was typed, when the layout flips between computer and phone", () => {
    seed("s1", [session], [project])
    const { rerender } = render(<AgentEnvDialog />)
    const env = screen.getByRole("textbox", { name: "Environment variables" })
    fireEvent.change(env, { target: { value: "FOO=1" } })
    env.focus()
    phone = true
    rerender(<AgentEnvDialog />)
    expect(screen.getByRole("textbox", { name: "Environment variables" })).toBe(env)
    expect(document.activeElement).toBe(env)
    phone = false
    rerender(<AgentEnvDialog />)
    expect(screen.getByRole("textbox", { name: "Environment variables" })).toBe(env)
    expect(document.activeElement).toBe(env)
  })

  it("draws the note about lines Save leaves out as the form's own 12px note, read with the box", () => {
    seed("s1", [session], [project])
    render(<AgentEnvDialog />)
    const env = screen.getByRole("textbox", { name: "Environment variables" })
    fireEvent.change(env, { target: { value: "not a variable" } })
    const note = document.getElementById(env.getAttribute("aria-describedby") ?? "")!
    expect(note.textContent).toMatch(/Save leaves it out/)
    expect(note.className).toContain("text-xs")
    expect(note.className).not.toContain("text-sm")
  })
})
