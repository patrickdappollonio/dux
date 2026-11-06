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
 * on no remote and no local branch that stays, or why git could not count;
 * `null` when git counted no such commit, since there is nothing to warn
 * about. Mirrors dux-core's `branch_loss_prose`, pinned by the shared prose
 * fixture.
 */
export function branchLossProse(
  branches: readonly string[],
  lost: { count: number; has_remote_refs: boolean } | { failed: string },
): Prose | null {
  if ("failed" in lost) return unpushedCountFailureProse([...branches], lost.failed)
  if (lost.count === 0) return null
  const [nowhere, tail] = lost.has_remote_refs
    ? ["on no remote and no branch that stays", ""]
    : ["on no branch that stays", ", and this repository has no remote"]
  const said =
    lost.count === 1
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
  return ["Looking for unpushed work on the branches…"]
}

/** What git answered about the commits a box would delete, or why it could not count. */
export type UnpushedAnswer =
  | { count: number; has_remote_refs: boolean }
  | { failed: string }

/**
 * What git answered about the branches the boxes offer (`current`, the first
 * box's, when there is one, and `others`), said before anything is ticked:
 * whether deleting them would lose work that exists nowhere else. The count is
 * the one a tick of every offered box would lose, so it agrees with the loss
 * warning; `failed` is the sentence for a count git could not make.
 *
 * The commits are said to be on the branches whose own box's count is not
 * zero, or on every offered branch when only their union holds them. More
 * than two such branches are counted rather than named, and the second box
 * counts as a whole, since git counts it as one. `ok: null` is no box offering
 * a branch, which has nothing to say. Mirrors dux-core's
 * `branches_checked_prose`, pinned by the shared prose fixture.
 */
export function branchesCheckedProse(
  current: string | null,
  currentLost: UnpushedAnswer,
  others: readonly string[],
  othersLost: UnpushedAnswer,
  bothLost: UnpushedAnswer,
): { ok: Prose | null } | { failed: Prose } {
  const offered = [...(current === null ? [] : [current]), ...others]
  if (offered.length === 0) return { ok: null }
  const lost =
    current !== null ? (others.length > 0 ? bothLost : currentLost) : othersLost
  if ("failed" in lost)
    return { failed: unpushedCountFailureProse(offered, lost.failed) }
  const noRemote = !lost.has_remote_refs
  if (lost.count === 0) {
    const where: Prose =
      offered.length <= 2
        ? nameList(offered, noRemote ? " and " : " or ")
        : [
            noRemote
              ? `the ${offered.length} branches`
              : `any of the ${offered.length} branches`,
          ]
    return {
      ok: noRemote
        ? [
            "Everything on ",
            ...where,
            " is also on another branch, and this repository has no remote",
          ]
        : ["No unpushed work on ", ...where],
    }
  }
  const holds = (answer: UnpushedAnswer) => !("failed" in answer) && answer.count > 0
  const named = [
    ...(current !== null && holds(currentLost) ? [current] : []),
    ...(holds(othersLost) ? others : []),
  ]
  const holding = named.length > 0 ? named : offered
  const one = lost.count === 1
  const where: Prose =
    holding.length > 2 ? [`${holding.length} branches`] : nameList(holding, " and ")
  const tail = noRemote
    ? one
      ? " is on no other branch, and this repository has no remote"
      : " are on no other branch, and this repository has no remote"
    : one
      ? " isn't pushed anywhere"
      : " aren't pushed anywhere"
  return {
    ok: joinProse(
      [[one ? "1 commit on " : `${lost.count} commits on `], where, [tail]],
      "",
    ),
  }
}

/** What the branch boxes say when git could not be asked about them. */
export function branchesCheckFailedProse(reason: string): Prose {
  return [`dux could not check the branches (${reason}), so they will be kept.`]
}

/**
 * What the branch boxes say when git answered about every branch but the one
 * the worktree is on, which is the only one kept on that account.
 */
export function currentBranchCheckFailedProse(
  branch: string,
  reason: string,
): Prose {
  return ["dux could not check ", chip(branch), ` (${reason}), so it will be kept.`]
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
