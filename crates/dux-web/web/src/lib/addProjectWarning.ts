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

/**
 * The confirm-button label for the add-project warning, mirroring the TUI: when
 * the user opts to check out the default first it reads "Check Out & Add",
 * otherwise "Add Anyway".
 */
export function addConfirmLabel(checkoutDefault: boolean): string {
  return checkoutDefault ? "Check Out & Add" : "Add Anyway"
}
