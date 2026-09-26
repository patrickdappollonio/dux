// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { Spine } from "@/lib/types"

// Journeys through the Projects list with the REAL store and the real dialogs:
// only the HTTP boundary (fetch) is stubbed, and the spine is overlaid on the
// store's own state because it normally arrives over the events socket.
let spine: Spine
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: (): DuxState => ({ ...actual.useDux(), spine }),
  }
})

type FetchCall = { url: string; method: string }
let fetchCalls: FetchCall[] = []

// The store boots on import (localStorage + a bootstrap fetch), so both are
// stubbed before it loads. Every request is recorded and answered with an empty
// success, which is what the accepted project and terminal routes return.
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
    vi.fn((url: string, init?: RequestInit) => {
      fetchCalls.push({ url: String(url), method: init?.method ?? "GET" })
      return Promise.resolve(new Response(null, { status: 204 }))
    }),
  )
}
installBootStubs()

const store = await import("@/lib/store")
const { ProjectsDialog } = await import("./ProjectsDialog")
const { DeleteProjectDialog } = await import("./DeleteProjectDialog")
const { ProjectSettingsDialog } = await import("./ProjectSettingsDialog")
const { RemoveProjectDialog } = await import("./RemoveProjectDialog")

function project(id: string, name: string, extra: object = {}) {
  return {
    id,
    name,
    path: `/code/${name}`,
    default_provider: "claude",
    explicit_default_provider: null,
    auto_reopen_agents: null,
    startup_command: null,
    env: {},
    current_branch: "main",
    branch_status: "leading",
    path_missing: false,
    leading_branch: "main",
    created_at: "",
    ...extra,
  }
}

function agent(id: string, projectId: string, createdAt: string) {
  return {
    id,
    workspace: { kind: "managed", project_id: projectId },
    created_at: createdAt,
  }
}

// acme has an agent, beta has none, and "ghost" is an orphaned group: agents
// whose project record is gone.
function seedSpine(
  projects = [
    project("p1", "acme", { created_at: "2026-07-01T09:00:00Z" }),
    project("p2", "beta", {
      created_at: "2026-07-02T09:00:00Z",
      leading_branch: null,
    }),
  ],
  sessions = [agent("a1", "p1", "2026-07-05T09:00:00Z"), agent("g1", "gone", "")],
) {
  spine = {
    projects,
    sessions,
    terminals: [],
    sidebar: {
      groups: [
        {
          project_id: "gone",
          name: "ghost",
          orphaned: true,
          path_missing: true,
          session_ids: ["g1"],
        },
      ],
      agentless_start: null,
    },
  } as unknown as Spine
}

function App() {
  return (
    <>
      <ProjectsDialog />
      <DeleteProjectDialog />
      <RemoveProjectDialog />
      <ProjectSettingsDialog />
    </>
  )
}

function openList() {
  const view = render(<App />)
  act(() => store.openProjects())
  return view
}

function rowNames(): string[] {
  return screen
    .getAllByTestId("project-row")
    .map((row) => row.getAttribute("data-name") ?? "")
}

function row(name: string): HTMLElement {
  const found = screen
    .getAllByTestId("project-row")
    .find((r) => r.getAttribute("data-name") === name)
  if (!found) throw new Error(`no row named ${name}`)
  return found
}

function listIsOpen(): boolean {
  return screen.queryByLabelText("Search projects") !== null
}

beforeEach(() => {
  installBootStubs()
  fetchCalls = []
  seedSpine()
})

afterEach(() => {
  act(() => {
    store.closeProjects()
    store.closeDeleteProject()
    store.closeRemoveProject()
    store.closeProjectSettings()
  })
  cleanup()
  vi.unstubAllGlobals()
})

describe("ProjectsDialog", () => {
  it("lists an agent-less project and an orphaned group beside the rest", () => {
    openList()
    expect(screen.getByRole("heading", { name: "Projects" })).toBeTruthy()
    expect(rowNames()).toEqual(["acme", "beta", "ghost"])
    // Each row carries its folder, agent count and base branch.
    const beta = row("beta")
    expect(beta.textContent).toContain("/code/beta")
    expect(beta.textContent).toContain("0 agents")
    expect(beta.textContent).toContain("no base yet")
    expect(row("acme").textContent).toContain("1 agent")
    expect(row("acme").textContent).toContain("main")
  })

  it("opens Delete project over the list, and cancelling returns to it", async () => {
    openList()
    fireEvent.click(
      within(row("beta")).getByRole("button", { name: "Project actions" }),
    )
    fireEvent.click(await screen.findByText("Delete project…"))

    expect(await screen.findByText("Delete project?")).toBeTruthy()
    // The list is still underneath.
    expect(listIsOpen()).toBe(true)

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(screen.queryByText("Delete project?")).toBeNull()
    expect(listIsOpen()).toBe(true)
    expect(rowNames()).toContain("beta")
  })

  it("offers an orphaned group only Remove project", async () => {
    openList()
    fireEvent.click(
      within(row("ghost")).getByRole("button", { name: "Project actions" }),
    )
    await screen.findByRole("menu")
    expect(
      screen.getAllByRole("menuitem").map((item) => item.textContent),
    ).toEqual(["Remove project…"])
  })

  // The whole row is a target: a click anywhere on it opens the same menu the
  // ⋯ does.
  it("opens the row's menu when the row itself is clicked", async () => {
    openList()
    fireEvent.click(within(row("beta")).getByRole("button", { name: /beta/ }))
    expect(await screen.findByText("Project settings…")).toBeTruthy()
  })

  it("closes the list for New terminal, and keeps it under Project settings", async () => {
    openList()
    fireEvent.click(within(row("beta")).getByRole("button", { name: /beta/ }))
    fireEvent.click(await screen.findByText("Project settings…"))
    expect(await screen.findByText(/Project settings:/)).toBeTruthy()
    expect(listIsOpen()).toBe(true)
    act(() => store.closeProjectSettings())
    expect(listIsOpen()).toBe(true)

    fireEvent.click(within(row("beta")).getByRole("button", { name: /beta/ }))
    fireEvent.click(await screen.findByText("New terminal at the project root"))
    expect(listIsOpen()).toBe(false)
    expect(
      fetchCalls.some(
        (call) => call.method === "POST" && call.url.includes("/p2/terminals"),
      ),
    ).toBe(true)
  })

  it("narrows the rows by search, keeps the order frozen, and drops a removed project", () => {
    const { rerender } = openList()
    fireEvent.change(screen.getByLabelText("Search projects"), {
      target: { value: "bet" },
    })
    expect(rowNames()).toEqual(["beta"])
    fireEvent.change(screen.getByLabelText("Search projects"), {
      target: { value: "" },
    })

    // beta gets the newest agent: a live sort would lift it above acme.
    seedSpine(undefined, [
      agent("a1", "p1", "2026-07-05T09:00:00Z"),
      agent("a2", "p2", "2026-07-09T09:00:00Z"),
      agent("g1", "gone", ""),
    ])
    rerender(<App />)
    expect(rowNames()).toEqual(["acme", "beta", "ghost"])

    // acme is removed while the list is open.
    seedSpine([project("p2", "beta")], [agent("g1", "gone", "")])
    rerender(<App />)
    expect(rowNames()).toEqual(["beta", "ghost"])
  })

  it("says a project's folder is missing", () => {
    seedSpine([project("p1", "acme", { path_missing: true })], [])
    openList()
    expect(row("acme").textContent).toContain("Folder missing")
  })
})
