import { beforeEach, describe, expect, it, vi } from "vitest"

const openAddProject = vi.fn()
const openAddProjectForInit = vi.fn()
const openCreateAgentFromPr = vi.fn()
const openNewAgentPicker = vi.fn()
const createStandaloneTerminal = vi.fn()

vi.mock("@/lib/store", () => ({
  openAddProject: () => openAddProject(),
  openAddProjectForInit: () => openAddProjectForInit(),
  openCreateAgentFromPr: (projectId: string | null) =>
    openCreateAgentFromPr(projectId),
  openNewAgentPicker: (intent: string) => openNewAgentPicker(intent),
  createStandaloneTerminal: () => createStandaloneTerminal(),
}))

import {
  addProjectMenuItems,
  newMenuItems,
  splitCreationGroups,
  NEW_AGENT_PLAIN_ID,
  type CreationMenuItem,
} from "@/lib/creationMenus"

// The action rows only; the separator has no title to assert against.
const actions = (entries: CreationMenuItem[]) =>
  entries.filter((e) => e.kind === "item")

describe("newMenuItems", () => {
  beforeEach(() => vi.clearAllMocks())

  it("lists the agent variants, then a rule, then the standalone terminal", () => {
    expect(
      newMenuItems({ ghAvailable: true }).map((e) => [e.kind, e.id]),
    ).toEqual([
      ["item", "new-agent-plain"],
      ["item", "new-agent-from-pr"],
      ["item", "new-agent-from-worktree"],
      ["item", "new-standalone-agent"],
      ["separator", "sep-new-terminals"],
      ["item", "new-standalone-terminal"],
    ])
    expect(actions(newMenuItems({ ghAvailable: true })).map((i) => i.title)).toEqual([
      "New agent…",
      "New agent from PR…",
      "New agent from existing worktree…",
      "New standalone agent…",
      "New standalone terminal in your home folder",
    ])
  })

  it("hides the from-PR variant when gh is unavailable", () => {
    expect(newMenuItems({ ghAvailable: false }).map((e) => e.id)).toEqual([
      "new-agent-plain",
      "new-agent-from-worktree",
      "new-standalone-agent",
      "sep-new-terminals",
      "new-standalone-terminal",
    ])
  })

  it("exports the plain-agent id the launcher corner filters its ⋯ menu by", () => {
    // The corner's filled verb IS that action, so the ⋯ menu drops exactly this
    // id. Pinning the constant against the list keeps the filter from quietly
    // matching nothing after a rename.
    expect(NEW_AGENT_PLAIN_ID).toBe("new-agent-plain")
    expect(
      newMenuItems({ ghAvailable: true }).some(
        (e) => e.id === NEW_AGENT_PLAIN_ID,
      ),
    ).toBe(true)
  })

  it("carries a trailing ellipsis iff the item opens a dialog", () => {
    for (const item of actions(newMenuItems({ ghAvailable: true }))) {
      // The one exception, stated rather than excluded: the standalone terminal
      // opens immediately, with no dialog and nothing to confirm.
      const opensDialog = item.id !== "new-standalone-terminal"
      expect(
        item.title.endsWith("…"),
        `${item.id} should ${opensDialog ? "" : "not "}end with …`,
      ).toBe(opensDialog)
      expect(item.icon, `${item.id} needs a lucide icon`).toBeDefined()
    }
  })

  it("routes each entry to the same store action the sidebar's control calls", () => {
    const items = actions(newMenuItems({ ghAvailable: true }))
    items.find((i) => i.id === "new-agent-plain")!.run()
    expect(openNewAgentPicker).toHaveBeenCalledWith("new")
    items.find((i) => i.id === "new-agent-from-pr")!.run()
    expect(openCreateAgentFromPr).toHaveBeenCalledWith(null)
    items.find((i) => i.id === "new-agent-from-worktree")!.run()
    expect(openNewAgentPicker).toHaveBeenCalledWith("from_worktree")
    items.find((i) => i.id === "new-standalone-terminal")!.run()
    expect(createStandaloneTerminal).toHaveBeenCalledOnce()
  })
})

describe("splitCreationGroups", () => {
  it("splits the New list at its rule into an agents chunk and a terminals chunk", () => {
    expect(
      splitCreationGroups(newMenuItems({ ghAvailable: true })).map((chunk) =>
        chunk.map((i) => i.id),
      ),
    ).toEqual([
      [
        "new-agent-plain",
        "new-agent-from-pr",
        "new-agent-from-worktree",
        "new-standalone-agent",
      ],
      ["new-standalone-terminal"],
    ])
  })

  it("keeps the chunk positions when a gate hides an item", () => {
    // Position is what the labels key off, so a hidden from-PR variant must
    // shrink the agents chunk rather than shift the terminals one.
    expect(
      splitCreationGroups(newMenuItems({ ghAvailable: false })).map((chunk) =>
        chunk.map((i) => i.id),
      ),
    ).toEqual([
      ["new-agent-plain", "new-agent-from-worktree", "new-standalone-agent"],
      ["new-standalone-terminal"],
    ])
  })

  it("drops every separator row and carries only actions through", () => {
    for (const chunk of splitCreationGroups(newMenuItems({ ghAvailable: true }))) {
      for (const entry of chunk) expect(entry.kind).toBe("item")
    }
  })

  it("keeps an empty chunk rather than renumbering the ones after it", () => {
    // Hand-built input: no shipped list looks like this today, and the renderer
    // relies on chunk N staying chunk N if one ever does.
    expect(
      splitCreationGroups([
        { kind: "separator", id: "sep-a" },
        ...addProjectMenuItems(),
      ]).map((chunk) => chunk.length),
    ).toEqual([0, 2])
  })

  it("returns one chunk for a list with no separators at all", () => {
    expect(splitCreationGroups(addProjectMenuItems())).toHaveLength(1)
  })
})

describe("addProjectMenuItems", () => {
  beforeEach(() => vi.clearAllMocks())

  it("lists the two add-project variants in the sidebar's order", () => {
    expect(actions(addProjectMenuItems()).map((i) => i.title)).toEqual([
      "Add project…",
      "Initialize a repository…",
    ])
  })

  it("titles every item with a trailing ellipsis and gives it an icon", () => {
    for (const item of actions(addProjectMenuItems())) {
      expect(item.title.endsWith("…"), `${item.id} should end with …`).toBe(true)
      expect(item.icon, `${item.id} needs a lucide icon`).toBeDefined()
    }
  })

  it("routes each variant to the same store action the sidebar's menu calls", () => {
    const items = actions(addProjectMenuItems())
    items.find((i) => i.id === "add-project-picker")!.run()
    expect(openAddProject).toHaveBeenCalledOnce()
    items.find((i) => i.id === "init-repository")!.run()
    expect(openAddProjectForInit).toHaveBeenCalledOnce()
  })
})
