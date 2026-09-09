// Branch-warning copy and decision helpers for the add-project pre-flight. The strings
// are byte-for-byte the TUI's `ConfirmNonDefaultBranch` lines, joined into prose for the
// web dialog. Keep them in step with crates/dux-tui/src/app/render.rs.

import type { BranchWarningView, InspectKind } from "./types"

export interface BranchWarningCopy {
  // The headline sentence describing the situation.
  message: string
  // The always-present note that new worktrees fork from the current branch.
  worktreeNote: string
  // The dim explanatory note shown only on the heuristic path; null otherwise.
  heuristicNote: string | null
  // True when the warning offers a "check out the default branch first" action
  // (only the Known variant, matching the TUI's checkbox availability).
  canCheckoutDefault: boolean
  // The default branch name, present only on the Known variant.
  defaultBranch: string | null
}

/**
 * Map a branch warning + current branch to the exact user-facing copy and the
 * available choices, mirroring the TUI's `ConfirmNonDefaultBranch` rendering.
 */
export function branchWarningCopy(
  warning: BranchWarningView,
  currentBranch: string,
): BranchWarningCopy {
  const worktreeNote = `New worktrees will branch from "${currentBranch}".`
  if (warning.kind === "known") {
    return {
      message: `This repository is on branch ${currentBranch}, but the remote default branch is ${warning.default_branch}.`,
      worktreeNote,
      heuristicNote: null,
      canCheckoutDefault: true,
      defaultBranch: warning.default_branch,
    }
  }
  return {
    message: `This repository is on branch ${currentBranch}, which doesn't appear to be the main branch.`,
    worktreeNote,
    heuristicNote:
      "Dux can't confidently identify this repo's default branch, so it won't change branches for you.",
    canCheckoutDefault: false,
    defaultBranch: null,
  }
}

export interface NoCommitsCopy {
  // Headline: the repo has no commits yet.
  message: string
  // Reassurance: the commit dux makes is empty and won't touch existing files.
  note: string
}

/**
 * Copy for the unborn-HEAD case: a repo with no commits cannot back a worktree
 * until it has a root commit, so dux offers an empty initial commit.
 */
export function noCommitsCopy(): NoCommitsCopy {
  return {
    message:
      "This repository has no commits yet, so agents can't branch worktrees from it.",
    note: "Dux will make an empty initial commit; your existing files are left untouched (untracked).",
  }
}

export interface InitRepoCopy {
  // Headline: the folder is not a git repository.
  message: string
  // What dux will do: init, seed (when candidates exist), empty commit.
  note: string
}

/**
 * Copy for the plain-folder case: dux offers to initialize a repository. The seed
 * clause is omitted when there are no candidates, never promising a seed that will not happen.
 */
export function initRepoCopy(candidates: string[]): InitRepoCopy {
  const seedClause =
    candidates.length > 0
      ? `, seed a starter .gitignore covering ${candidates.join(", ")},`
      : ""
  return {
    message: "This folder is not a git repository.",
    note: `Dux will run git init${seedClause} and make an empty initial commit; your existing files are left untouched (untracked).`,
  }
}

export interface InsideRepoCopy {
  message: string
}

/**
 * Copy for the blocked case: the folder sits inside an existing repository. A null
 * `root` means git's internal directory, and the copy degrades to not naming a root.
 */
export function insideRepoCopy(root: string | null): InsideRepoCopy {
  if (root) {
    return {
      message: `This folder is inside the git repository at ${root}. Add that repository instead.`,
    }
  }
  return {
    message:
      "This folder is inside a git repository's internal directory. Add the repository itself instead.",
  }
}

export type AddProjectAction =
  | "plain"
  | "checkout-default"
  | "initial-commit"
  | "init-repo"
  | "blocked"

export interface AddProjectPrimaryAction {
  action: AddProjectAction
  label: string
}

/**
 * The add dialog's primary action and button label, in precedence order:
 * `blocked` (a folder inside a repository) outranks everything; a `plain` kind
 * outranks `hasCommits`, because init subsumes the commit; an unborn repo
 * outranks a branch warning, because there is no default branch to check out.
 */
export function addProjectPrimaryAction(opts: {
  kind: InspectKind
  hasCommits: boolean
  willCheckout: boolean
  hasBranchWarning: boolean
}): AddProjectPrimaryAction {
  if (opts.kind === "repo_subdir") {
    return { action: "blocked", label: "Add project" }
  }
  if (opts.kind === "plain") {
    return { action: "init-repo", label: "Initialize Repository & Add" }
  }
  if (!opts.hasCommits) {
    return { action: "initial-commit", label: "Create Initial Commit & Add" }
  }
  if (opts.willCheckout) {
    return { action: "checkout-default", label: "Check Out & Add" }
  }
  if (opts.hasBranchWarning) {
    return { action: "plain", label: "Add Anyway" }
  }
  return { action: "plain", label: "Add project" }
}
