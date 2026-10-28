// The branch sentences a delete confirmation shows beside its "also delete the
// branch" box: the box's label, and why a branch dux did not create deserves a
// second look. Shared by the agent-delete dialog and the worktree manager's
// left-over folder dialog, which offer the same choice about the same kind of
// branch.

import { chip, type Prose } from "@/lib/prose"

// Why a branch predates the agent, one clause per provenance; an unrecognized
// provenance gets its own, since nothing here can claim the branch is older.
// `subject` names the branch once there are two on screen.
function existedBeforeSentence(provenance: string, subject: Prose): Prose {
  if (provenance === "adopted")
    return [...subject, " came with the worktree this agent adopted."]
  if (provenance === "unknown") return [...subject, " is not one dux created."]
  return [...subject, " existed before the agent."]
}

// The predates sentence for `branches`, or `null` for a branch dux created.
// With two branches the second is the one the agent was born on, which is the
// one its provenance describes, so it is named; with one it is "This branch".
export function predatesSentence(
  provenance: string,
  branches: string[],
): Prose | null {
  if (provenance === "created") return null
  return existedBeforeSentence(
    provenance,
    branches.length > 1 ? [chip(branches[1])] : ["This branch"],
  )
}

// The branch checkbox's label, naming EVERY branch the tick would delete: a box
// promising one deletion and performing two takes consent it was not given.
export function branchCheckboxLabel(branches: string[]): Prose {
  if (branches.length === 0) return []
  if (branches.length === 1) return ["Also delete the branch ", chip(branches[0])]
  return [
    "Also delete the branches ",
    ...branches.flatMap((branch, index): Prose =>
      index === 0 ? [chip(branch)] : [" and ", chip(branch)],
    ),
  ]
}
