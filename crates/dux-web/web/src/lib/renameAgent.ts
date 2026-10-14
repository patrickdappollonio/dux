// The rename dialog's rules, kept pure so the dialog, the store and the tests
// read the same answers. The terminal UI's dialog follows the same rules
// through `dux_core::engine::rename_agent`.

import { sanitizeAgentName } from "./agentName"
import { chip, type Prose } from "./prose"

/**
 * The line under the "Also rename the git branch" box. Renaming moves the
 * branch on this machine only, so when it has an open pull request or a pushed
 * copy the line says which of them stays on the old name.
 *
 * dux-core's `rename_branch_note_prose` builds the same segments for the
 * terminal UI, and both are pinned by
 * `crates/dux-core/tests/fixtures/prose_cross_language.json`.
 */
export function renameBranchNoteProse(
  branch: string,
  openPr: number | null,
  pushed: boolean,
): Prose {
  const lead: Prose = ["Renames ", chip(branch)]
  if (openPr !== null) {
    return [
      ...lead,
      " on this machine only: pull request ",
      chip(`#${openPr}`),
      pushed
        ? " and the pushed branch stay on the old name."
        : " stays on the old name.",
    ]
  }
  if (pushed) {
    return [
      ...lead,
      " on this machine only: the pushed branch on the remote keeps the old name.",
    ]
  }
  return [...lead, " to the new name."]
}

/** What the name field holds after a keystroke. Only a name that can become a
 * branch is held to the refname characters; a standalone agent's name is a
 * label and is kept exactly as typed. */
export function renameDraftValue(raw: string, branchNamed: boolean): string {
  return branchNamed ? sanitizeAgentName(raw) : raw
}

/** Why the dialog cannot confirm, or `null` when it can. */
export function renameBlockedReason(
  draft: string,
  renameBranch: boolean,
  branchNamed: boolean,
): string | null {
  if (branchNamed && renameBranch && draft.trim() === "") {
    return "Type a name to rename the branch to. An empty name only resets the agent's name."
  }
  return null
}

/** The PATCH body the dialog sends. The branch flag means nothing for an agent
 * without a branch, so it is never sent true for one. */
export function renamePayload(
  draft: string,
  renameBranch: boolean,
  branchNamed: boolean,
): { title: string; rename_branch: boolean } {
  return { title: draft.trim(), rename_branch: branchNamed && renameBranch }
}
