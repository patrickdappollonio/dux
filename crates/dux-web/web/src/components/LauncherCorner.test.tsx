// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

const openNewAgentPicker = vi.fn()
const openAddProject = vi.fn()
const openCreateAgentFromPr = vi.fn()
const createStandaloneTerminal = vi.fn()
// The corner reads gh availability and the project count from the live store;
// tests flip these. `projects: null` stands for a spine that has not arrived.
let ghAvailable = true
let projects: { id: string }[] | null = [{ id: "p1" }]
vi.mock("@/lib/store", () => ({
  openNewAgentPicker: (intent: string) => openNewAgentPicker(intent),
  openAddProject: () => openAddProject(),
  openCreateAgentFromPr: (projectId: string | null) =>
    openCreateAgentFromPr(projectId),
  createStandaloneTerminal: () => createStandaloneTerminal(),
  openAddProjectForInit: vi.fn(),
  useDux: () => ({
    bootstrap: { gh_available: ghAvailable },
    spine: projects === null ? null : { projects },
  }),
}))

import { LauncherCorner } from "@/components/LauncherCorner"

const overflow = () =>
  screen.getByRole("button", { name: /^more ways to create$/i })

const openMenu = async () => {
  fireEvent.click(overflow())
  await screen.findByRole("menu")
}

beforeEach(() => {
  ghAvailable = true
  projects = [{ id: "p1" }]
  vi.clearAllMocks()
})
afterEach(() => cleanup())

describe("LauncherCorner verb", () => {
  it("is New agent while a project exists, and opens the picker", () => {
    render(<LauncherCorner />)
    fireEvent.click(screen.getByRole("button", { name: /^new agent$/i }))
    expect(openNewAgentPicker).toHaveBeenCalledWith("new")
    expect(openAddProject).not.toHaveBeenCalled()
  })

  it("flips to Add project on a confirmed-empty workspace, and opens that picker", () => {
    projects = []
    render(<LauncherCorner />)
    expect(screen.queryByRole("button", { name: /^new agent$/i })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /^add project$/i }))
    expect(openAddProject).toHaveBeenCalledOnce()
    expect(openNewAgentPicker).not.toHaveBeenCalled()
  })

  // The no-flash rule: an unloaded spine is not a zero, so a populated
  // workspace never renders "Add project" for the frame before its spine lands.
  it("stays New agent while the spine has not arrived", () => {
    projects = null
    render(<LauncherCorner />)
    expect(screen.getByRole("button", { name: /^new agent$/i })).toBeTruthy()
  })

  it("keeps the same ⋯ in both verb states", async () => {
    render(<LauncherCorner />)
    await openMenu()
    const withProject = screen.getAllByRole("menuitem").map((i) => i.textContent)
    cleanup()

    projects = []
    render(<LauncherCorner />)
    await openMenu()
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(
      withProject,
    )
    // The plain-agent row stays filtered out even here: the menu is constant by
    // design, so the flipped state duplicates "Add project…" rather than
    // reshuffling rows under the cursor.
    expect(screen.queryByText("New agent…")).toBeNull()
    expect(screen.getByText("Add project…")).toBeTruthy()
  })
})

describe("LauncherCorner overflow", () => {
  it("offers every other way to create, under the three group labels", async () => {
    render(<LauncherCorner />)
    await openMenu()
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "New agent from PR…",
      "New agent from existing worktree…",
      "New standalone agent…",
      "New standalone terminal",
      "Add project…",
      "Initialize a repository…",
    ])
    for (const label of ["Agents", "Terminals", "Projects"]) {
      expect(screen.getByText(label)).toBeTruthy()
    }
    // The verb beside it IS the plain agent; offering it twice would be a menu
    // row that does what the button next to it does.
    expect(screen.queryByText("New agent…")).toBeNull()
  })

  it("routes the menu entries to their store actions", async () => {
    render(<LauncherCorner />)
    await openMenu()
    fireEvent.click(screen.getByText("New standalone terminal"))
    expect(createStandaloneTerminal).toHaveBeenCalledOnce()

    await openMenu()
    fireEvent.click(screen.getByText("New agent from existing worktree…"))
    expect(openNewAgentPicker).toHaveBeenCalledWith("from_worktree")

    await openMenu()
    fireEvent.click(screen.getByText("New agent from PR…"))
    expect(openCreateAgentFromPr).toHaveBeenCalledWith(null)

    await openMenu()
    fireEvent.click(screen.getByText("Add project…"))
    expect(openAddProject).toHaveBeenCalledOnce()
  })
})

// The size-token contract the two controls share: one height token, the touch
// floor only where a finger is the pointer, and a square ⋯ on desktop that
// still clears 44px on touch.
describe("LauncherCorner sizing", () => {
  it("gives the verb and the ⋯ one height and the phone floor", () => {
    render(<LauncherCorner />)
    const verb = screen.getByRole("button", { name: /^new agent$/i })
    const trigger = overflow()
    for (const el of [verb, trigger]) {
      expect(el.className).toContain("h-7")
      expect(el.className).toContain("max-md:min-h-11")
    }
    expect(trigger.className).toContain("min-w-7")
    expect(trigger.className).toContain("max-md:min-w-11")
  })

  // Two separate rounded buttons with a gap, not a seam-joined group: the seam
  // (and its base-ui focus-guard rounding hack) died with the split buttons.
  it("renders two gapped buttons rather than a button group", () => {
    const { container } = render(<LauncherCorner />)
    expect(container.querySelector('[data-slot="button-group"]')).toBeNull()
    const row = container.firstElementChild as HTMLElement
    expect(row.className).toContain("gap-2")
    expect(row.querySelectorAll("button")).toHaveLength(2)
  })
})
