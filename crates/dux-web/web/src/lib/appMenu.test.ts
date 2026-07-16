import { beforeEach, describe, expect, it, vi } from "vitest"

const openCustomizeWebapp = vi.fn()
const openConfigEditor = vi.fn()
const openMacrosDialog = vi.fn()
const openGlobalEnv = vi.fn()
const openTaskManager = vi.fn()
const sortAgents = vi.fn()
const reload = vi.fn(() => Promise.resolve())

vi.mock("@/lib/store", () => ({
  openCustomizeWebapp: () => openCustomizeWebapp(),
  openConfigEditor: () => openConfigEditor(),
  openMacrosDialog: () => openMacrosDialog(),
  openGlobalEnv: () => openGlobalEnv(),
  openTaskManager: () => openTaskManager(),
  sortAgents: (by: string) => sortAgents(by),
}))
vi.mock("@/lib/configApi", () => ({ configApi: { reload: () => reload() } }))

import { SquarePen } from "lucide-react"

import {
  appMenuModel,
  findSubmenu,
  type AppMenuEntry,
} from "@/lib/appMenu"

// Depth-first walk of every entry in the tree, submenu children included.
function walk(entries: AppMenuEntry[]): AppMenuEntry[] {
  return entries.flatMap((e) =>
    e.kind === "submenu" ? [e, ...walk(e.entries)] : [e],
  )
}

describe("appMenuModel", () => {
  beforeEach(() => vi.clearAllMocks())

  it("lists the approved top-level entries in order", () => {
    expect(appMenuModel().map((e) => [e.kind, e.id])).toEqual([
      ["item", "preferences"],
      ["separator", "sep-preferences"],
      ["submenu", "sort-agents"],
      ["submenu", "configuration"],
      ["separator", "sep-agents"],
      ["item", "task-manager"],
    ])
  })

  it("titles every entry that opens a dialog or confirms with a trailing ellipsis", () => {
    const titleOf = (id: string) =>
      walk(appMenuModel()).find((e) => e.id === id && e.kind !== "separator") as
        | { title: string }
        | undefined

    for (const id of [
      "preferences",
      "edit-config",
      "edit-macros",
      "global-env",
      "task-manager",
    ]) {
      expect(titleOf(id)?.title.endsWith("…"), `${id} should end with …`).toBe(
        true,
      )
    }
    // Reload config runs immediately: no dialog, no confirmation, no ellipsis.
    expect(titleOf("reload-config")?.title).toBe("Reload config")
  })

  it("gives every item and submenu a leading icon", () => {
    for (const entry of walk(appMenuModel())) {
      if (entry.kind === "separator") continue
      expect(entry.icon, `${entry.id} needs a lucide icon`).toBeDefined()
    }
  })

  it("opens the sort submenu with the three approved options in order", () => {
    const sub = findSubmenu(appMenuModel(), "sort-agents")
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
    const sub = findSubmenu(appMenuModel(), "sort-agents")
    for (const entry of sub?.entries ?? []) {
      expect(entry.kind).toBe("item")
      expect(entry).not.toHaveProperty("checked")
      expect(entry).not.toHaveProperty("selected")
    }
  })

  it("orders the Configuration submenu with a separator before Reload config", () => {
    const sub = findSubmenu(appMenuModel(), "configuration")
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
    const sub = findSubmenu(appMenuModel(), "configuration")
    const editMacros = sub?.entries.find((e) => e.id === "edit-macros")
    expect(editMacros?.kind === "item" && editMacros.icon).toBe(SquarePen)
  })

  it("uses unique ids across the whole tree", () => {
    const ids = walk(appMenuModel()).map((e) => e.id)
    expect(new Set(ids).size).toBe(ids.length)
  })

  it("routes each item to its store action", () => {
    const run = (id: string) => {
      const entry = walk(appMenuModel()).find((e) => e.id === id)
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
    for (const entry of walk(appMenuModel())) {
      const text = `${entry.id} ${entry.kind === "separator" ? "" : entry.title}`
      expect(text).not.toMatch(
        /copy on select|copy-on-select|pr banner|pr-banner|tab strip|tab-strip|changes pane|changes-pane|github|pet name|pet-name/i,
      )
    }
  })
})

describe("findSubmenu", () => {
  it("returns null for an unknown id", () => {
    expect(findSubmenu(appMenuModel(), "nope")).toBeNull()
  })

  it("finds a submenu nested inside another submenu", () => {
    const nested: AppMenuEntry[] = [
      {
        kind: "submenu",
        id: "outer",
        title: "Outer",
        icon: findSubmenu(appMenuModel(), "configuration")!.icon,
        entries: [
          {
            kind: "submenu",
            id: "inner",
            title: "Inner",
            icon: findSubmenu(appMenuModel(), "configuration")!.icon,
            entries: [],
          },
        ],
      },
    ]
    expect(findSubmenu(nested, "inner")?.title).toBe("Inner")
  })
})
