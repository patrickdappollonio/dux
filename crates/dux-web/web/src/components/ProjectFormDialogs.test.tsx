// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

// The project-scoped forms an agent's or a project's menu opens. On a phone each
// is a full-height sheet with its own close button, and none focuses a text
// field by itself, because that raises the keyboard over the form.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
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
const { AgentStartupCommandDialog } = await import("./AgentStartupCommandDialog")
const { ProjectSettingsDialog } = await import("./ProjectSettingsDialog")

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
const project = {
  id: "p1",
  name: "acme",
  env: {},
  startup_command: null,
  explicit_default_provider: null,
  auto_reopen_agents: null,
}

beforeEach(() => {
  installBootStubs()
  mockState = {
    spine: { sessions: [session], projects: [project] },
    bootstrap: { available_providers: [] },
    agentEnvTarget: "s1",
    agentStartupCommandTarget: "s1",
    projectSettingsTarget: "p1",
  } as unknown as DuxState
})
afterEach(() => {
  cleanup()
  phone = false
  vi.unstubAllGlobals()
})

describe.each([
  ["the agent's environment", AgentEnvDialog, "KEY=VALUE"],
  ["the agent's startup command", AgentStartupCommandDialog, "npm run dev"],
])("%s", (_name, Dialog, placeholder) => {
  it("puts the caret in the text on a computer", () => {
    render(<Dialog />)
    expect(document.activeElement).toBe(screen.getByPlaceholderText(placeholder))
  })

  it("does not focus the text on a phone, and closes from its own button", () => {
    phone = true
    render(<Dialog />)
    expect(document.activeElement).not.toBe(screen.getByPlaceholderText(placeholder))
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy()
  })
})

describe("project settings", () => {
  it("is a sheet that closes from its own button on a phone", () => {
    phone = true
    render(<ProjectSettingsDialog />)
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy()
    expect(document.activeElement).not.toBe(screen.getByPlaceholderText("KEY=VALUE"))
  })
})
