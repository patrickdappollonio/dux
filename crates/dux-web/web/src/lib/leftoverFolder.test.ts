import { describe, expect, it } from "vitest"

import { normalizeLeftover } from "./leftoverFolder"
import type { LeftoverFolderWire } from "./types"

describe("a left-over folder from an older server", () => {
  const wire = (overrides: Partial<LeftoverFolderWire>): LeftoverFolderWire => ({
    path: "/wt/acme/stuck",
    branches: [],
    branch_provenance: null,
    in_use: null,
    folder_present: false,
    ...overrides,
  })

  it("reads what left it off the provenance", () => {
    expect(normalizeLeftover(wire({})).left_by).toBe("manager")
    expect(normalizeLeftover(wire({ branch_provenance: "attached" })).left_by).toBe(
      "agent",
    )
  })

  it("offers a delete only when there is a folder or a branch to delete", () => {
    expect(normalizeLeftover(wire({})).offers_delete).toBe(false)
    expect(normalizeLeftover(wire({ branches: ["feat"] })).offers_delete).toBe(true)
    expect(normalizeLeftover(wire({ folder_present: true })).offers_delete).toBe(true)
    expect(normalizeLeftover(wire({})).undeletable).toEqual([])
  })

  it("words a reason this build does not know as a branch git could not check", () => {
    const folder = normalizeLeftover(
      wire({
        undeletable: [
          { name: "feat", why: "frozen" as "moved" },
        ],
      }),
    )
    expect(folder.undeletable).toEqual([
      { name: "feat", why: "unchecked", reason: "frozen" },
    ])
  })
})
