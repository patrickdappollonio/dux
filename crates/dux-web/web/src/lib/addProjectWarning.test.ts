import { describe, expect, it } from "vitest"

import {
  addProjectPrimaryAction,
  branchWarningCopy,
  noCommitsCopy,
} from "./addProjectWarning"

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

describe("noCommitsCopy", () => {
  it("explains the repo has no commits and that an empty commit will be made", () => {
    const copy = noCommitsCopy()
    expect(copy.message).toContain("no commits")
    // The commit is empty and leaves existing files untouched/untracked.
    expect(copy.note).toContain("empty")
  })
})

describe("addProjectPrimaryAction", () => {
  it("offers to create the initial commit when the repo has none, taking precedence over branch warnings", () => {
    const action = addProjectPrimaryAction({
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("initial-commit")
    expect(action.label).toBe("Create Initial Commit & Add")
  })

  it("checks out the default first when the user opted in", () => {
    const action = addProjectPrimaryAction({
      hasCommits: true,
      willCheckout: true,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("checkout-default")
    expect(action.label).toBe("Check Out & Add")
  })

  it("reads 'Add Anyway' for a branch warning without checkout", () => {
    const action = addProjectPrimaryAction({
      hasCommits: true,
      willCheckout: false,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("plain")
    expect(action.label).toBe("Add Anyway")
  })

  it("reads 'Add project' for a clean repo on its default branch", () => {
    const action = addProjectPrimaryAction({
      hasCommits: true,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("plain")
    expect(action.label).toBe("Add project")
  })
})
