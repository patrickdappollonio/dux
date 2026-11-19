import { describe, expect, it } from "vitest"

import { FOLD_SLACK_PX, nextFolded } from "./use-fold-to-fit"

// Folding gives the names exactly the room the controls took, so the decision
// must not undo itself on the next measurement at the boundary width.
describe("nextFolded", () => {
  const room = { namesWidth: 300, clusterWidth: 310 }

  it("folds the moment a name is cut off, and not before", () => {
    expect(nextFolded(false, { ...room, rowWidth: 290, truncated: true })).toBe(true)
    expect(nextFolded(false, { ...room, rowWidth: 300, truncated: false })).toBe(false)
  })

  it("stays folded until the names and the controls fit with room to spare", () => {
    // The row the names had when they were cut off, plus the controls' width.
    const justFolded = 290 + room.clusterWidth
    expect(nextFolded(true, { ...room, rowWidth: justFolded, truncated: false })).toBe(true)
    const exactFit = room.namesWidth + room.clusterWidth
    expect(nextFolded(true, { ...room, rowWidth: exactFit, truncated: false })).toBe(true)
    expect(
      nextFolded(true, { ...room, rowWidth: exactFit + FOLD_SLACK_PX, truncated: false }),
    ).toBe(false)
  })

  it("stays folded while a name is cut off even with the controls gone", () => {
    expect(nextFolded(true, { ...room, rowWidth: 2000, truncated: true })).toBe(true)
  })
})
