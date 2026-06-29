import { beforeAll, describe, expect, it, vi } from "vitest"

import {
  GROUPED_PALETTE_IDS,
  PALETTE_GROUP_ORDER,
  groupPaletteCommands,
  paletteGroupFor,
} from "./paletteGroups"
import type { PaletteCommandView } from "./types"

// `paletteRegistry` imports `store`, which at module load reads `location` and
// `localStorage` and registers a `popstate` listener (the test env is node, not
// a DOM). Stub the minimum surface so the dynamic import succeeds.
beforeAll(() => {
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
})

// The web-surfaced command ids are derived from the registry itself
// (`PALETTE_HANDLERS` keys) rather than re-declared here — that handler set is
// pinned to the Rust core registry by `paletteRegistry.test.ts`, so this file
// inherits the same authoritative set without a third hand-mirrored copy.
// Grouping must cover exactly this set — a new Web/Both command must be mapped
// to a group.
async function webCommandIds(): Promise<string[]> {
  const { PALETTE_HANDLERS } = await import("./paletteRegistry")
  return Object.keys(PALETTE_HANDLERS)
}

function cmd(id: string): PaletteCommandView {
  return { id, description: `desc for ${id}` }
}

describe("paletteGroups", () => {
  it("maps every web-surfaced command id to a known group", async () => {
    for (const id of await webCommandIds()) {
      const group = paletteGroupFor(id)
      expect(group, `group for ${id}`).not.toBeNull()
      expect(PALETTE_GROUP_ORDER).toContain(group!)
    }
  })

  it("returns null for an unmapped id (no crash on a stale id)", () => {
    expect(paletteGroupFor("does-not-exist")).toBeNull()
  })

  it("has no stale group mapping (every grouped id is a live web command)", async () => {
    const live = new Set(await webCommandIds())
    for (const id of GROUPED_PALETTE_IDS) {
      expect(live.has(id), `stale group mapping for ${id}`).toBe(true)
    }
  })

  it("buckets commands into groups in the configured group order", async () => {
    const commands = (await webCommandIds()).map(cmd)
    const grouped = groupPaletteCommands(commands)
    expect(grouped.map((g) => g.group)).toEqual([
      "Configuration",
      "View",
      "Runtime",
      "Projects",
    ])
  })

  it("preserves the input order within a group", () => {
    // Reverse the View commands; the bucket should keep that relative order.
    const commands = [
      cmd("sort-agents-by-name"),
      cmd("sort-agents-by-created"),
      cmd("sort-agents-by-updated"),
    ]
    const grouped = groupPaletteCommands(commands)
    const view = grouped.find((g) => g.group === "View")
    expect(view?.commands.map((c) => c.id)).toEqual([
      "sort-agents-by-name",
      "sort-agents-by-created",
      "sort-agents-by-updated",
    ])
  })

  it("drops empty groups", () => {
    const grouped = groupPaletteCommands([cmd("add-project")])
    expect(grouped.map((g) => g.group)).toEqual(["Projects"])
  })

  it("ignores ids with no group mapping", () => {
    const grouped = groupPaletteCommands([cmd("add-project"), cmd("mystery")])
    const all = grouped.flatMap((g) => g.commands.map((c) => c.id))
    expect(all).toEqual(["add-project"])
  })
})
