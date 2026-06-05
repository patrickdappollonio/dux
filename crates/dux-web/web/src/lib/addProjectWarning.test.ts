import { describe, expect, it } from "vitest"

import { addConfirmLabel, branchWarningCopy } from "./addProjectWarning"

describe("branchWarningCopy", () => {
  it("names the default branch and offers checkout for a known warning", () => {
    const copy = branchWarningCopy(
      { kind: "known", default_branch: "main" },
      "feature/x",
    )
    expect(copy.message).toBe(
      "This repository is on branch feature/x, but the remote default branch is main.",
    )
    expect(copy.worktreeNote).toBe('New worktrees will branch from "feature/x".')
    expect(copy.heuristicNote).toBeNull()
    expect(copy.canCheckoutDefault).toBe(true)
    expect(copy.defaultBranch).toBe("main")
  })

  it("warns without offering checkout for a heuristic warning", () => {
    const copy = branchWarningCopy({ kind: "heuristic" }, "dev")
    expect(copy.message).toBe(
      "This repository is on branch dev, which doesn't appear to be the main branch.",
    )
    expect(copy.worktreeNote).toBe('New worktrees will branch from "dev".')
    expect(copy.heuristicNote).toBe(
      "Dux can't confidently identify this repo's default branch, so it won't change branches for you.",
    )
    expect(copy.canCheckoutDefault).toBe(false)
    expect(copy.defaultBranch).toBeNull()
  })
})

describe("addConfirmLabel", () => {
  it("reads 'Check Out & Add' when checking out the default first", () => {
    expect(addConfirmLabel(true)).toBe("Check Out & Add")
  })

  it("reads 'Add Anyway' otherwise", () => {
    expect(addConfirmLabel(false)).toBe("Add Anyway")
  })
})
