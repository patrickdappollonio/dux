// Decides how a project row renders its current git branch. Render-only: it never
// dispatches. The view model carries `current_branch` and `branch_status` but not the
// project's leading branch NAME, so the non-leading tooltip cannot name it.

import type { ProjectView } from "./types"

export interface ProjectBranchDisplay {
  // The branch text to render after the project name.
  branch: string
  // Whether to tint it with the warning token (the project isn't on its
  // leading branch). A "leading" or "unknown" status is shown muted/secondary.
  warn: boolean
  // A one-line explanation for the row's title/tooltip, or null when nothing
  // extra is worth saying (the branch text already speaks for itself).
  tooltip: string | null
}

// Decide the branch display for a project row, or null when there's nothing to
// render (empty/unknown branch, e.g. path_missing projects) so the caller emits
// no span at all rather than an empty one.
export function projectBranchDisplay(
  project: Pick<ProjectView, "current_branch" | "branch_status">,
): ProjectBranchDisplay | null {
  const branch = project.current_branch.trim()
  if (branch === "") return null
  const warn = project.branch_status === "not_leading"
  return {
    branch,
    warn,
    tooltip: warn
      ? `On ${branch} — this doesn't appear to be the project's leading branch.`
      : null,
  }
}
