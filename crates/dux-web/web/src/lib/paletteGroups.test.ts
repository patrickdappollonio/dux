import { describe, expect, it } from "vitest"

import {
  PALETTE_GROUP_ORDER,
  groupPaletteCommands,
  paletteGroupFor,
} from "./paletteGroups"
import type { PaletteCommandView } from "./types"

// The web-surfaced command ids, mirrored from `EXPECTED_WEB_COMMANDS` in
// `paletteRegistry.test.ts` (itself pinned to the Rust registry). Grouping must
// cover exactly this set — a new Web/Both command must be mapped to a group.
const EXPECTED_WEB_COMMANDS = [
  "add-project",
  "configure-global-env",
  "edit-macros",
  "reload-config",
  "sort-agents-by-created",
  "sort-agents-by-name",
  "sort-agents-by-updated",
  "toggle-diff-line-numbers",
]

function cmd(id: string): PaletteCommandView {
  return { id, description: `desc for ${id}` }
}

describe("paletteGroups", () => {
  it("maps every web-surfaced command id to a known group", () => {
    for (const id of EXPECTED_WEB_COMMANDS) {
      const group = paletteGroupFor(id)
      expect(group, `group for ${id}`).not.toBeNull()
      expect(PALETTE_GROUP_ORDER).toContain(group!)
    }
  })

  it("returns null for an unmapped id (no crash on a stale id)", () => {
    expect(paletteGroupFor("does-not-exist")).toBeNull()
  })

  it("buckets commands into groups in the configured group order", () => {
    const commands = EXPECTED_WEB_COMMANDS.map(cmd)
    const grouped = groupPaletteCommands(commands)
    expect(grouped.map((g) => g.group)).toEqual([
      "Configuration",
      "View",
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
