import { beforeEach, describe, expect, it, vi } from "vitest"

const openAddProject = vi.fn()
const openAddProjectForInit = vi.fn()
const openCreateAgentFromPr = vi.fn()
const openNewAgentPicker = vi.fn()

vi.mock("@/lib/store", () => ({
  openAddProject: () => openAddProject(),
  openAddProjectForInit: () => openAddProjectForInit(),
  openCreateAgentFromPr: (projectId: string | null) =>
    openCreateAgentFromPr(projectId),
  openNewAgentPicker: (intent: string) => openNewAgentPicker(intent),
}))

import { addProjectMenuItems, newAgentMenuItems } from "@/lib/creationMenus"

describe("newAgentMenuItems", () => {
  beforeEach(() => vi.clearAllMocks())

  it("lists the three creation variants in the sidebar's order when gh is available", () => {
    expect(newAgentMenuItems({ ghAvailable: true }).map((i) => i.title)).toEqual([
      "New agent…",
      "New agent from PR…",
      "New agent from existing worktree…",
    ])
  })

  it("hides the from-PR variant when gh is unavailable", () => {
    expect(newAgentMenuItems({ ghAvailable: false }).map((i) => i.id)).toEqual([
      "new-agent-plain",
      "new-agent-from-worktree",
    ])
  })

  it("titles every item with a trailing ellipsis because each opens a dialog", () => {
    for (const item of newAgentMenuItems({ ghAvailable: true })) {
      expect(item.title.endsWith("…"), `${item.id} should end with …`).toBe(true)
      expect(item.icon, `${item.id} needs a lucide icon`).toBeDefined()
    }
  })

  it("routes each variant to the same store action the sidebar's control calls", () => {
    const items = newAgentMenuItems({ ghAvailable: true })
    items.find((i) => i.id === "new-agent-plain")!.run()
    expect(openNewAgentPicker).toHaveBeenCalledWith("new")
    items.find((i) => i.id === "new-agent-from-pr")!.run()
    expect(openCreateAgentFromPr).toHaveBeenCalledWith(null)
    items.find((i) => i.id === "new-agent-from-worktree")!.run()
    expect(openNewAgentPicker).toHaveBeenCalledWith("from_worktree")
  })
})

describe("addProjectMenuItems", () => {
  beforeEach(() => vi.clearAllMocks())

  it("lists the two add-project variants in the sidebar's order", () => {
    expect(addProjectMenuItems().map((i) => i.title)).toEqual([
      "Add project…",
      "Initialize a repository…",
    ])
  })

  it("titles every item with a trailing ellipsis and gives it an icon", () => {
    for (const item of addProjectMenuItems()) {
      expect(item.title.endsWith("…"), `${item.id} should end with …`).toBe(true)
      expect(item.icon, `${item.id} needs a lucide icon`).toBeDefined()
    }
  })

  it("routes each variant to the same store action the sidebar's menu calls", () => {
    const items = addProjectMenuItems()
    items.find((i) => i.id === "add-project-picker")!.run()
    expect(openAddProject).toHaveBeenCalledOnce()
    items.find((i) => i.id === "init-repository")!.run()
    expect(openAddProjectForInit).toHaveBeenCalledOnce()
  })
})
