// What the delete-agent dialog says about the branches its two boxes offer,
// built in step with dux-core's `delete_agent_prose` for the terminal UI.

import { chip, joinProse, type Prose } from "./prose"

/**
 * The sentence saying the unpushed commits on `branches` could not be counted,
 * with git's `reason`. Every branch is a name; the reason is relayed error text
 * and stays plain. Two branches (a drifted agent) take the plural.
 *
 * dux-core's `unpushed_count_failure_prose` builds the same segments for the
 * terminal UI, and both are pinned by
 * `crates/dux-core/tests/fixtures/prose_cross_language.json`.
 */
export function unpushedCountFailureProse(
  branches: string[],
  reason: string,
): Prose {
  return [
    "dux could not count the unpushed commits on ",
    ...branches.flatMap((branch, index): Prose =>
      index === 0 ? [chip(branch)] : [" and ", chip(branch)],
    ),
    ` (${reason}); `,
    branches.length > 1
      ? "check them before deleting them."
      : "check it before deleting it.",
  ]
}

/** Who made a branch the delete dialog offers, as the server judged it. */
export type BranchOrigin =
  | "dux"
  | "attached"
  | "adopted"
  | "not_dux"
  | "after"
  | "before"
  | "unknown"

// `[a]`, `[a] and [b]`, `[a], [b] and [c]` with `joiner` before the last.
function nameList(names: readonly string[], joiner: string): Prose {
  return names.flatMap((name, index): Prose => {
    if (index === 0) return [chip(name)]
    return [index + 1 === names.length ? joiner : ", ", chip(name)]
  })
}

// Sentences joined by one space, with adjacent words merged the way
// `dux_core::prose` merges them.
function sentences(parts: readonly Prose[]): Prose {
  return joinProse(
    parts.filter((part) => part.length > 0),
    " ",
  )
}

/** The first box's label. */
export function currentBranchLabelProse(branch: string): Prose {
  return ["Also delete the branch ", chip(branch)]
}

/** The second box's label, naming every branch a tick deletes. */
export function otherBranchesLabelProse(branches: readonly string[]): Prose {
  return joinProse(
    [
      [
        branches.length === 1
          ? "Also delete the other branch this agent was on: "
          : "Also delete the other branches this agent was on: ",
      ],
      nameList(branches, " and "),
    ],
    "",
  )
}

/**
 * What deleting `branches` (every box ticked right now) loses: their commits
 * on no remote and no local branch that stays, or why git could not count.
 * Mirrors dux-core's `branch_loss_prose`, pinned by the shared prose fixture.
 */
export function branchLossProse(
  branches: readonly string[],
  lost: { count: number; has_remote_refs: boolean } | { failed: string },
): Prose {
  if ("failed" in lost) return unpushedCountFailureProse([...branches], lost.failed)
  const [also, nowhere, tail] = lost.has_remote_refs
    ? [
        "on a remote or a branch that stays",
        "on no remote and no branch that stays",
        "",
      ]
    : [
        "on a branch that stays",
        "on no branch that stays",
        ", and this repository has no remote",
      ]
  const said =
    lost.count === 0
      ? ` loses no commits: every one is also ${also}.`
      : lost.count === 1
        ? ` loses 1 commit that is ${nowhere}${tail}.`
        : ` loses ${lost.count} commits that are ${nowhere}${tail}.`
  return ["Deleting ", ...nameList(branches, " and "), said]
}

/**
 * The warning under the first box, `null` for a branch dux can prove is its
 * own. An origin this page has never heard of reads as `unknown`: nothing here
 * can claim more about it.
 */
export function currentBranchWarningProse(origin: BranchOrigin): Prose | null {
  let reason: string | null
  switch (origin) {
    case "dux":
      return null
    case "after":
      reason = null
      break
    case "attached":
    case "before":
      reason = "This branch existed before the agent."
      break
    case "adopted":
      reason = "This branch came with the worktree this agent adopted."
      break
    case "not_dux":
      reason = "This branch is not one dux created."
      break
    default:
      reason =
        "dux cannot tell when this branch was made, so it may not be this agent's."
  }
  return reason === null ? null : [reason]
}

/**
 * The warning under the second box: who dux cannot vouch for, and what
 * unticking leaves.
 */
export function otherBranchesWarningProse(
  others: readonly { name: string; origin: BranchOrigin }[],
  ticked: boolean,
): Prose {
  const known = new Set<string>([
    "dux",
    "attached",
    "adopted",
    "not_dux",
    "after",
    "before",
    "unknown",
  ])
  const named = (pick: (origin: BranchOrigin) => boolean): string[] =>
    others
      .filter((other) =>
        pick(known.has(other.origin) ? other.origin : "unknown"),
      )
      .map((other) => other.name)
  const parts: Prose[] = []
  const before = named((o) => o === "attached" || o === "before")
  if (before.length > 0)
    parts.push([...nameList(before, " and "), " existed before the agent."])
  const adopted = named((o) => o === "adopted")
  if (adopted.length > 0)
    parts.push([
      ...nameList(adopted, " and "),
      " came with the worktree this agent adopted.",
    ])
  const notDux = named((o) => o === "not_dux")
  if (notDux.length > 0)
    parts.push([
      ...nameList(notDux, " and "),
      notDux.length === 1
        ? " is not one dux created."
        : " are not ones dux created.",
    ])
  const after = named((o) => o === "after")
  if (after.length > 0)
    parts.push([
      "dux did not make ",
      ...nameList(after, " or "),
      ", and cannot tell who did.",
    ])
  const unknown = named((o) => o === "unknown")
  if (unknown.length > 0)
    parts.push([
      "dux cannot tell when ",
      ...nameList(unknown, " and "),
      unknown.length === 1 ? " was made." : " were made.",
    ])
  const many = others.length > 1
  if (!ticked)
    parts.push([
      many
        ? "Unticked, they stay as ordinary branches."
        : "Unticked, it stays as an ordinary branch.",
    ])
  return sentences(parts)
}

/** What the branch boxes say while the dialog is still asking git about them. */
export function branchesCheckingProse(): Prose {
  return ["Checking the branches…"]
}

/** What the branch boxes say when git could not be asked about them. */
export function branchesCheckFailedProse(reason: string): Prose {
  return [`dux could not check the branches (${reason}), so they will be kept.`]
}

/**
 * The line naming branches the agent was on that are the project's own, which
 * no box offers and dux never deletes. Mirrors dux-core's
 * `protected_branches_prose`, pinned by the shared prose fixture.
 */
export function protectedBranchesProse(branches: string[]): Prose {
  return [
    ...nameList(branches, " and "),
    branches.length === 1
      ? " is the project's own branch, so dux keeps it."
      : " are the project's own branches, so dux keeps them.",
  ]
}
