import { describe, expect, it } from "vitest"

import { changeBaseBranchProse } from "./changeBaseBranch"
import { proseText } from "./prose"

// The plain-text spelling is pinned to literal strings; the segments are pinned
// against the terminal UI's by the shared fixture in prose.test.tsx.
describe("changeBaseBranchProse", () => {
  it("names the folder's new branch and where worktrees branch from before and after", () => {
    expect(proseText(changeBaseBranchProse("dux", "main", "develop"))).toBe(
      'This switches the source checkout for "dux" to "develop", moving HEAD in the shared repository. New worktrees branch from "main" now. After the switch, they branch from "develop".',
    )
  })

  it("says the base stays put when the chosen branch already is the base", () => {
    expect(proseText(changeBaseBranchProse("dux", "develop", "develop"))).toBe(
      'This switches the source checkout for "dux" to "develop", moving HEAD in the shared repository. New worktrees already branch from "develop", and still will after the switch.',
    )
  })

  it("says the project has no base yet when none is recorded", () => {
    expect(proseText(changeBaseBranchProse("dux", null, "develop"))).toBe(
      'This switches the source checkout for "dux" to "develop", moving HEAD in the shared repository. The project has no base branch recorded yet. After the switch, new worktrees branch from "develop".',
    )
  })
})
