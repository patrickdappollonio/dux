// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

// The project row's ⋯ menu is where the PROJECT scope of the startup-command log
// viewer lives (the agent row's menu carries the agent scope). What is asserted
// here is the wiring and, just as importantly, the WORDING: the two entries sit
// one row-menu apart and must not read alike.

const openProjectStartupLogs = vi.fn()
vi.mock("@/lib/store", () => ({
  createProjectTerminal: vi.fn(),
  openAttachWorktree: vi.fn(),
  openCheckoutDefaultBranch: vi.fn(),
  openCreateAgent: vi.fn(),
  openCreateAgentFromPr: vi.fn(),
  openDeleteProject: vi.fn(),
  openProjectInfo: vi.fn(),
  openProjectSettings: vi.fn(),
  openProjectStartupLogs: (id: string) => openProjectStartupLogs(id),
  openRemoveProject: vi.fn(),
  pullProject: vi.fn(),
  useDux: () => ({
    bootstrap: { gh_available: false },
    spine: { projects: [{ id: "p1", name: "Repo", path_missing: false }] },
  }),
}))

const { ProjectMenuItems } = await import("@/components/ProjectMenuItems")
const {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} = await import("@/components/ui/dropdown-menu")

function openMenu() {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <ProjectMenuItems id="p1" />
      </DropdownMenuContent>
    </DropdownMenu>,
  )
  fireEvent.click(screen.getByText("open"))
  return screen.findByRole("menu")
}

afterEach(() => {
  cleanup()
  openProjectStartupLogs.mockClear()
})

describe("ProjectMenuItems startup-command logs entry", () => {
  it("offers a project-scoped log entry that cannot be read as the agent one", async () => {
    await openMenu()
    const item = screen.getByText("Startup command logs for all agents…")
    expect(item).toBeTruthy()
    // The agent menu's entry is the bare "Startup command logs…"; an exact-text
    // query for it must find nothing here.
    expect(screen.queryByText("Startup command logs…")).toBeNull()
  })

  it("keeps the leading icon and the trailing ellipsis the menu conventions require", async () => {
    await openMenu()
    const item = screen
      .getByText("Startup command logs for all agents…")
      .closest('[role="menuitem"]')
    expect(item).toBeTruthy()
    // Trailing "…" marks an item that opens a dialog; a leading lucide icon is
    // required on every item in these menus.
    expect(item!.textContent?.endsWith("…")).toBe(true)
    expect(item!.querySelector("svg")).toBeTruthy()
  })

  it("routes the entry to the project-scope store action with the project id", async () => {
    await openMenu()
    fireEvent.click(screen.getByText("Startup command logs for all agents…"))
    expect(openProjectStartupLogs).toHaveBeenCalledWith("p1")
  })
})
