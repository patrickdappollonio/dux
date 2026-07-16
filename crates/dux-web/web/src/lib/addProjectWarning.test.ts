import { describe, expect, it } from "vitest"

import {
  addProjectPrimaryAction,
  branchWarningCopy,
  initRepoCopy,
  insideRepoCopy,
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

describe("initRepoCopy", () => {
  it("names the candidates dux will seed", () => {
    const copy = initRepoCopy(["node_modules", ".venv"])
    expect(copy.message).toBe("This folder is not a git repository.")
    expect(copy.note).toContain("git init")
    expect(copy.note).toContain("node_modules, .venv")
    expect(copy.note).toContain("empty initial commit")
  })

  it("omits the seed clause entirely when there are no candidates", () => {
    // Never promise a seed that will not happen.
    const copy = initRepoCopy([])
    expect(copy.note).not.toContain(".gitignore")
    expect(copy.note).toContain("git init")
  })
})

describe("insideRepoCopy", () => {
  it("names the enclosing repository root", () => {
    const copy = insideRepoCopy("/home/u/repo")
    expect(copy.message).toBe(
      "This folder is inside the git repository at /home/u/repo. Add that repository instead.",
    )
  })

  it("degrades gracefully when no root can be named (git-internal dir)", () => {
    const copy = insideRepoCopy(null)
    expect(copy.message).toContain("internal directory")
    expect(copy.message).not.toContain("null")
  })
})

describe("addProjectPrimaryAction", () => {
  it("blocks a repo subdirectory, outranking everything (even hasCommits: false)", () => {
    // A wrong rung here means the wrong wire flag, i.e. the wrong server
    // mutation (an initial commit inside someone's repo subfolder).
    const action = addProjectPrimaryAction({
      kind: "repo_subdir",
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("blocked")
  })

  it("offers to initialize a repository for a plain folder", () => {
    const action = addProjectPrimaryAction({
      kind: "plain",
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("init-repo")
    expect(action.label).toBe("Initialize Repository & Add")
  })

  it("offers to create the initial commit when the repo has none, taking precedence over branch warnings", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("initial-commit")
    expect(action.label).toBe("Create Initial Commit & Add")
  })

  it("checks out the default first when the user opted in", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: true,
      willCheckout: true,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("checkout-default")
    expect(action.label).toBe("Check Out & Add")
  })

  it("reads 'Add Anyway' for a branch warning without checkout", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: true,
      willCheckout: false,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("plain")
    expect(action.label).toBe("Add Anyway")
  })

  it("reads 'Add project' for a clean repo on its default branch", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: true,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("plain")
    expect(action.label).toBe("Add project")
  })
})
