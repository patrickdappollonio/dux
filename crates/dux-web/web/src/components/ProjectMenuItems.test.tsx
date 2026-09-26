// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

// The project row's ⋯ menu is where the PROJECT scope of the startup-command log
// viewer lives (the agent row's menu carries the agent scope). What is asserted
// here is the wiring and, just as importantly, the WORDING: the two entries sit
// one row-menu apart and must not read alike.

const openProjectStartupLogs = vi.fn()
const openChangeBaseBranch = vi.fn()
const createProjectTerminal = vi.fn()
let pathMissing = false
vi.mock("@/lib/store", () => ({
  createProjectTerminal: (id: string) => createProjectTerminal(id),
  openAttachWorktree: vi.fn(),
  openChangeBaseBranch: (id: string) => openChangeBaseBranch(id),
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
    spine: {
      projects: [{ id: "p1", name: "Repo", path_missing: pathMissing }],
    },
  }),
}))

const { ProjectMenuItems } = await import("@/components/ProjectMenuItems")
const {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} = await import("@/components/ui/dropdown-menu")

function openMenu(onLeave?: () => void) {
  render(
    <DropdownMenu>
      <DropdownMenuTrigger>open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <ProjectMenuItems id="p1" onLeave={onLeave} />
      </DropdownMenuContent>
    </DropdownMenu>,
  )
  fireEvent.click(screen.getByText("open"))
  return screen.findByRole("menu")
}

afterEach(() => {
  cleanup()
  openProjectStartupLogs.mockClear()
  openChangeBaseBranch.mockClear()
  createProjectTerminal.mockClear()
  pathMissing = false
})

function items(): HTMLElement[] {
  return screen.getAllByRole("menuitem")
}

describe("ProjectMenuItems as a whole", () => {
  it("lists the project actions in the order both surfaces share", async () => {
    await openMenu()
    expect(items().map((item) => item.textContent)).toEqual([
      "New agent…",
      "Worktrees…",
      "New terminal at the project root",
      "Pull project",
      "Check out default branch…",
      "Change base branch…",
      "Project info…",
      "Project settings…",
      "Startup command logs for all agents…",
      "Delete project…",
      "Remove project…",
    ])
  })

  // Pull project runs at once and opens nothing, so it carries no "…".
  it("reads Pull project with no trailing ellipsis", async () => {
    await openMenu()
    expect(screen.getByText("Pull project")).toBeTruthy()
    expect(screen.queryByText("Pull project…")).toBeNull()
  })

  it("gives every item a leading icon", async () => {
    await openMenu()
    for (const item of items()) {
      expect(
        item.firstElementChild?.tagName.toLowerCase(),
        `${item.textContent} needs a leading icon`,
      ).toBe("svg")
    }
  })

  it("opens Change base branch for the project", async () => {
    await openMenu()
    fireEvent.click(screen.getByText("Change base branch…"))
    expect(openChangeBaseBranch).toHaveBeenCalledWith("p1")
  })

  // Both need the folder: there is no root to open a shell at and no checkout
  // to switch.
  it("disables the folder-bound items when the folder is missing", async () => {
    pathMissing = true
    await openMenu()
    for (const label of [
      "New terminal at the project root",
      "Change base branch…",
    ]) {
      expect(
        screen
          .getByText(label)
          .closest('[role="menuitem"]')
          ?.getAttribute("aria-disabled"),
      ).toBe("true")
    }
  })

  // A surface that stays behind (the Projects list) is told first, so it can
  // get out of the way of an item that takes the user somewhere else.
  it("tells the host before an item that leaves it, and not before one that opens over it", async () => {
    const onLeave = vi.fn()
    await openMenu(onLeave)
    fireEvent.click(screen.getByText("New terminal at the project root"))
    expect(onLeave).toHaveBeenCalledTimes(1)
    expect(createProjectTerminal).toHaveBeenCalledWith("p1")
    cleanup()
    onLeave.mockClear()
    await openMenu(onLeave)
    fireEvent.click(screen.getByText("Change base branch…"))
    expect(onLeave).not.toHaveBeenCalled()
  })
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

// The verb form, matching the confirmation's title and its button.
describe("ProjectMenuItems default-branch entry", () => {
  it("reads Check out, never the noun Checkout", async () => {
    await openMenu()
    expect(screen.getByText("Check out default branch…")).toBeTruthy()
    expect(screen.queryByText("Checkout default branch…")).toBeNull()
  })
})
