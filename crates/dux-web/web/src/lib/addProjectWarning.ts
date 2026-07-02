// Branch-warning copy + decision helpers for the add-project pre-flight, ported
// from the TUI's `ConfirmNonDefaultBranch` dialog (dux-tui render.rs / input.rs).
//
// The TUI inspects the repo's current branch before adding and, when HEAD is not
// the default branch, shows a warning with two variants:
//   - Known     (origin/HEAD resolved a default that differs from HEAD): names
//                the default branch and OFFERS to check it out before adding
//                ("Check Out & Add" — the TUI defaults this on).
//   - Heuristic (origin/HEAD unavailable, HEAD is not main/master): warns but
//                CANNOT offer a switch, because dux can't identify the default.
//
// The strings below are byte-for-byte the TUI's lines, joined into prose for the
// web dialog. Keep them in sync with crates/dux-tui/src/app/render.rs.

import type { BranchWarningView } from "./types"

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
 * Copy for the unborn-HEAD case: a freshly `git init`'d repo has no commits, so
 * it can't back a worktree until it has a root commit. dux offers to create an
 * empty initial commit; existing files are left untracked.
 */
export function noCommitsCopy(): NoCommitsCopy {
  return {
    message:
      "This repository has no commits yet, so agents can't branch worktrees from it.",
    note: "Dux will make an empty initial commit — your existing files are left untouched (untracked).",
  }
}

export type AddProjectAction = "plain" | "checkout-default" | "initial-commit"

export interface AddProjectPrimaryAction {
  action: AddProjectAction
  label: string
}

/**
 * Decide the add dialog's primary action + button label from the inspection
 * state. An unborn repo (no commits) takes precedence over any branch warning:
 * there is no default branch to check out, and after the initial commit the
 * current branch simply becomes the leading branch.
 */
export function addProjectPrimaryAction(opts: {
  hasCommits: boolean
  willCheckout: boolean
  hasBranchWarning: boolean
}): AddProjectPrimaryAction {
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
