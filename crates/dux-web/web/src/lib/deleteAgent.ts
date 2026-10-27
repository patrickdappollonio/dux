// What the delete-agent dialog says when git could not count the commits its
// branch box would destroy.

import { chip, type Prose } from "./prose"

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
