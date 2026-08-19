import { beforeEach, describe, expect, it, vi } from "vitest"

const openCustomizeWebapp = vi.fn()
const openConfigEditor = vi.fn()
const openMacrosDialog = vi.fn()
const openGlobalEnv = vi.fn()
const openTaskManager = vi.fn()
const sortAgents = vi.fn()
const openWelcomeScreen = vi.fn()
const openReleaseNotes = vi.fn()
const openAddProject = vi.fn()
const openAddProjectForInit = vi.fn()
const openCreateAgentFromPr = vi.fn()
const openNewAgentPicker = vi.fn()
const createStandaloneTerminal = vi.fn()
const reload = vi.fn(() => Promise.resolve())

vi.mock("@/lib/store", () => ({
  openCustomizeWebapp: () => openCustomizeWebapp(),
  openConfigEditor: () => openConfigEditor(),
  openMacrosDialog: () => openMacrosDialog(),
  openGlobalEnv: () => openGlobalEnv(),
  openTaskManager: () => openTaskManager(),
  sortAgents: (by: string) => sortAgents(by),
  openWelcomeScreen: () => openWelcomeScreen(),
  openReleaseNotes: () => openReleaseNotes(),
  openAddProject: () => openAddProject(),
  openAddProjectForInit: () => openAddProjectForInit(),
  openCreateAgentFromPr: (projectId: string | null) =>
    openCreateAgentFromPr(projectId),
  openNewAgentPicker: (intent: string) => openNewAgentPicker(intent),
  createStandaloneTerminal: () => createStandaloneTerminal(),
}))
vi.mock("@/lib/configApi", () => ({ configApi: { reload: () => reload() } }))

import { SquarePen } from "lucide-react"

import {
  appMenuModel,
  findSubmenu,
  type AppMenuEntry,
} from "@/lib/appMenu"
import { addProjectMenuItems, newMenuItems } from "@/lib/creationMenus"

// Depth-first walk of every entry in the tree, submenu children included.
function walk(entries: AppMenuEntry[]): AppMenuEntry[] {
  return entries.flatMap((e) =>
    e.kind === "submenu" ? [e, ...walk(e.entries)] : [e],
  )
}

// Most tests build the fullest menu: gh available, so the from-PR entry exists.
const ctx = { ghAvailable: true }

describe("appMenuModel", () => {
  beforeEach(() => vi.clearAllMocks())

  it("lists the approved top-level entries in order", () => {
    expect(appMenuModel(ctx).map((e) => [e.kind, e.id])).toEqual([
      // The creation submenus open the menu: creating is the most common
      // reason to reach for the cog.
      ["submenu", "new-agent"],
      ["submenu", "add-project"],
      ["separator", "sep-create"],
      ["item", "preferences"],
      ["separator", "sep-preferences"],
      ["submenu", "sort-agents"],
      ["submenu", "configuration"],
      ["separator", "sep-agents"],
      // No top-level new-standalone-terminal: its one home is the New submenu
      // above (pinned as absent by the test below).
      ["item", "task-manager"],
      ["separator", "sep-about"],
      ["item", "welcome-screen"],
      ["item", "release-notes"],
    ])
  })

  // THE ANTI-DRIFT PIN for the creation submenus: their entries are the same
  // shared lists the launcher corner's ⋯ menu renders (creationMenus.ts), so
  // the cog menu and the sidebar cannot disagree about labels, icons, order, or
  // gating.
  it("builds the creation submenus from the shared sidebar lists", () => {
    // `run` is a fresh closure per construction, so compare everything else;
    // the run behavior itself is pinned by the store-routing test below.
    // Separators pass straight through, so the shape function has to handle
    // both arms of the shared list's union.
    const shape = (entries: AppMenuEntry[] | undefined) =>
      entries?.map((e) =>
        e.kind === "item"
          ? { kind: e.kind, id: e.id, title: e.title, icon: e.icon }
          : { kind: e.kind, id: e.id },
      )
    const sharedShape = (
      entries: ReturnType<typeof addProjectMenuItems>,
    ) =>
      entries.map((e) =>
        e.kind === "item"
          ? { kind: e.kind, id: e.id, title: e.title, icon: e.icon }
          : { kind: e.kind, id: e.id },
      )
    for (const ghAvailable of [true, false]) {
      const model = appMenuModel({ ghAvailable })
      const agentSub = findSubmenu(model, "new-agent")
      expect(agentSub?.title).toBe("New")
      expect(shape(agentSub?.entries)).toEqual(
        sharedShape(newMenuItems({ ghAvailable })),
      )
      // The cog keeps the whole list, separator and standalone terminal
      // included: unlike the launcher corner there is no adjacent verb here.
      expect(agentSub?.entries.map((e) => e.id)).toContain(
        "new-standalone-terminal",
      )
      const projectSub = findSubmenu(model, "add-project")
      expect(projectSub?.title).toBe("Add project")
      expect(shape(projectSub?.entries)).toEqual(
        sharedShape(addProjectMenuItems()),
      )
    }
  })

  // The standalone terminal used to ALSO sit at the top level. It has one home
  // now, so the cog cannot offer the same click twice.
  it("offers the standalone terminal only inside the New submenu", () => {
    const model = appMenuModel(ctx)
    expect(model.map((e) => e.id)).not.toContain("new-standalone-terminal")
    expect(walk(model).filter((e) => e.id === "new-standalone-terminal")).toHaveLength(
      1,
    )
  })

  it("routes the standalone terminal to its store action", () => {
    const entry = walk(appMenuModel(ctx)).find(
      (e) => e.id === "new-standalone-terminal",
    )
    if (entry?.kind !== "item") throw new Error("not an item")
    entry.run()
    expect(createStandaloneTerminal).toHaveBeenCalledOnce()
  })

  it("hides the from-PR agent variant when gh is unavailable", () => {
    const ids = (ghAvailable: boolean) =>
      walk(appMenuModel({ ghAvailable })).map((e) => e.id)
    expect(ids(true)).toContain("new-agent-from-pr")
    expect(ids(false)).not.toContain("new-agent-from-pr")
  })

  it("titles every entry that opens a dialog or confirms with a trailing ellipsis", () => {
    const titleOf = (id: string) =>
      walk(appMenuModel(ctx)).find((e) => e.id === id && e.kind !== "separator") as
        | { title: string }
        | undefined

    for (const id of [
      "preferences",
      "edit-config",
      "edit-macros",
      "global-env",
      "task-manager",
      // Every creation variant opens a dialog (picker or from-PR dialog).
      "new-agent-plain",
      "new-agent-from-pr",
      "new-agent-from-worktree",
      "add-project-picker",
      "init-repository",
      // Both first-load entries open the shared dialog.
      "welcome-screen",
      "release-notes",
    ]) {
      expect(titleOf(id)?.title.endsWith("…"), `${id} should end with …`).toBe(
        true,
      )
    }
    // Reload config runs immediately: no dialog, no confirmation, no ellipsis.
    expect(titleOf("reload-config")?.title).toBe("Reload config")
    // Same for the standalone terminal: it opens the terminal on the spot.
    expect(titleOf("new-standalone-terminal")?.title).toBe(
      "New standalone terminal",
    )
  })

  it("gives every item and submenu a leading icon", () => {
    for (const entry of walk(appMenuModel(ctx))) {
      if (entry.kind === "separator") continue
      expect(entry.icon, `${entry.id} needs a lucide icon`).toBeDefined()
    }
  })

  it("opens the sort submenu with the three approved options in order", () => {
    const sub = findSubmenu(appMenuModel(ctx), "sort-agents")
    expect(sub?.title).toBe("Sort agents by")
    expect(sub?.entries.map((e) => e.id)).toEqual([
      "sort-updated",
      "sort-created",
      "sort-name",
    ])
    expect(
      sub?.entries.map((e) => (e.kind === "item" ? e.title : null)),
    ).toEqual(["Recently updated", "Created", "Name"])
  })

  // `sortAgents` is a ONE-SHOT reorder: it computes an order and POSTs it as the
  // user's manual drag order. There is no persisted sort key to check against,
  // and a checkmark would be false the moment the user drags a row. So these are
  // plain action items, never radio items.
  it("renders the sort options as plain items with no selected-state indicator", () => {
    const sub = findSubmenu(appMenuModel(ctx), "sort-agents")
    for (const entry of sub?.entries ?? []) {
      expect(entry.kind).toBe("item")
      expect(entry).not.toHaveProperty("checked")
      expect(entry).not.toHaveProperty("selected")
    }
  })

  it("orders the Configuration submenu with a separator before Reload config", () => {
    const sub = findSubmenu(appMenuModel(ctx), "configuration")
    expect(sub?.title).toBe("Configuration")
    expect(sub?.entries.map((e) => e.id)).toEqual([
      "edit-config",
      "edit-macros",
      "global-env",
      "sep-config",
      "reload-config",
    ])
  })

  // The MacroPopover's own "Edit macros…" link (which calls the same
  // openMacrosDialog action) uses SquarePen, so the app menu item for the same
  // action must use the same icon rather than a mismatched one.
  it("uses the same icon as MacroPopover's Edit macros… link", () => {
    const sub = findSubmenu(appMenuModel(ctx), "configuration")
    const editMacros = sub?.entries.find((e) => e.id === "edit-macros")
    expect(editMacros?.kind === "item" && editMacros.icon).toBe(SquarePen)
  })

  it("uses unique ids across the whole tree", () => {
    const ids = walk(appMenuModel(ctx)).map((e) => e.id)
    expect(new Set(ids).size).toBe(ids.length)
  })

  it("routes each item to its store action", () => {
    const run = (id: string) => {
      const entry = walk(appMenuModel(ctx)).find((e) => e.id === id)
      if (entry?.kind !== "item") throw new Error(`${id} is not an item`)
      entry.run()
    }

    run("preferences")
    expect(openCustomizeWebapp).toHaveBeenCalledOnce()
    run("edit-config")
    expect(openConfigEditor).toHaveBeenCalledOnce()
    run("edit-macros")
    expect(openMacrosDialog).toHaveBeenCalledOnce()
    run("global-env")
    expect(openGlobalEnv).toHaveBeenCalledOnce()
    run("task-manager")
    expect(openTaskManager).toHaveBeenCalledOnce()
    run("reload-config")
    expect(reload).toHaveBeenCalledOnce()
    run("welcome-screen")
    expect(openWelcomeScreen).toHaveBeenCalledOnce()
    run("release-notes")
    expect(openReleaseNotes).toHaveBeenCalledOnce()

    run("new-agent-plain")
    expect(openNewAgentPicker).toHaveBeenCalledWith("new")
    run("new-agent-from-pr")
    expect(openCreateAgentFromPr).toHaveBeenCalledWith(null)
    run("new-agent-from-worktree")
    expect(openNewAgentPicker).toHaveBeenCalledWith("from_worktree")
    run("add-project-picker")
    expect(openAddProject).toHaveBeenCalledOnce()
    run("init-repository")
    expect(openAddProjectForInit).toHaveBeenCalledOnce()

    run("sort-updated")
    expect(sortAgents).toHaveBeenCalledWith("updated")
    run("sort-created")
    expect(sortAgents).toHaveBeenCalledWith("created")
    run("sort-name")
    expect(sortAgents).toHaveBeenCalledWith("name")
  })

  // The six preference-shaped toggles the web command palette used to carry are
  // settings, not actions: they live in the Preferences dialog now. This pins
  // that consolidation so one cannot quietly creep back into the menu.
  it("does not reference the removed palette toggles", () => {
    for (const entry of walk(appMenuModel(ctx))) {
      const text = `${entry.id} ${entry.kind === "separator" ? "" : entry.title}`
      expect(text).not.toMatch(
        /copy on select|copy-on-select|pr banner|pr-banner|tab strip|tab-strip|changes pane|changes-pane|github|pet name|pet-name/i,
      )
    }
  })
})

describe("findSubmenu", () => {
  it("returns null for an unknown id", () => {
    expect(findSubmenu(appMenuModel(ctx), "nope")).toBeNull()
  })

  it("finds a submenu nested inside another submenu", () => {
    const nested: AppMenuEntry[] = [
      {
        kind: "submenu",
        id: "outer",
        title: "Outer",
        icon: findSubmenu(appMenuModel(ctx), "configuration")!.icon,
        entries: [
          {
            kind: "submenu",
            id: "inner",
            title: "Inner",
            icon: findSubmenu(appMenuModel(ctx), "configuration")!.icon,
            entries: [],
          },
        ],
      },
    ]
    expect(findSubmenu(nested, "inner")?.title).toBe("Inner")
  })
})
