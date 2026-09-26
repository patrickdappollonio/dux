// Copy for the "Change base branch" confirmation on an existing project, built
// as prose (`./prose`) so the web draws the project and the branches as chips.

import { type Prose, quotedChip } from "./prose"

/**
 * The confirmation body: the project folder switches to `to`, and new
 * worktrees branch from `to` afterwards instead of `from`, the project's
 * recorded base (`ProjectView.leading_branch`, `null` when there is none yet).
 *
 * dux-core's `change_base_branch_confirm_prose` builds the same segments for
 * the terminal UI, and both are pinned by
 * `crates/dux-core/tests/fixtures/prose_cross_language.json`.
 */
export function changeBaseBranchProse(
  projectName: string,
  from: string | null,
  to: string,
): Prose {
  const lead: Prose = [
    "This switches the source checkout for ",
    quotedChip(projectName),
    " to ",
    quotedChip(to),
    ", moving HEAD in the shared repository.",
  ]
  if (from === to) {
    return [
      ...lead,
      " New worktrees already branch from ",
      quotedChip(to),
      ", and still will after the switch.",
    ]
  }
  if (from) {
    return [
      ...lead,
      " New worktrees branch from ",
      quotedChip(from),
      " now. After the switch, they branch from ",
      quotedChip(to),
      ".",
    ]
  }
  return [
    ...lead,
    " The project has no base branch recorded yet. After the switch, new worktrees branch from ",
    quotedChip(to),
    ".",
  ]
}
