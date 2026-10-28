// The sentences of the worktree manager's left-over folder delete dialog. Each
// is dux-core's `leftover_folders` sentence of the same name, pinned word for
// word by the shared prose fixture, so the terminal UI and the web say the
// same thing about the same folder.

import { chip, type Prose } from "@/lib/prose"
import { singleQuoted } from "@/lib/shellQuote"
import type {
  LeftoverFolderView,
  LeftoverFolderWire,
  UndeletableBranchView,
} from "@/lib/types"

const KNOWN_WHYS: ReadonlySet<string> = new Set<UndeletableBranchView["why"]>([
  "no_commit",
  "checked_out",
  "moved",
  "unchecked",
])

// A left-over folder from the wire, with what an older server does not send
// filled in: no branch it would keep, `left_by` read off the provenance (null
// only for the manager's own removal), and a delete offered when there is a
// folder or a branch to delete. A `why` this build does not know is worded as
// a branch git could not check, naming that `why`, rather than left unworded.
export function normalizeLeftover(wire: LeftoverFolderWire): LeftoverFolderView {
  return {
    ...wire,
    undeletable: (wire.undeletable ?? []).map((branch) =>
      KNOWN_WHYS.has(branch.why)
        ? branch
        : { name: branch.name, why: "unchecked", reason: String(branch.why) },
    ),
    left_by:
      wire.left_by ?? (wire.branch_provenance === null ? "manager" : "agent"),
    offers_delete:
      wire.offers_delete ?? (wire.folder_present || wire.branches.length > 0),
  }
}

// The names as one list: "a", "a and b", "a, b and c".
function namesProse(names: readonly string[]): Prose {
  return names.flatMap((name, index): Prose => {
    if (index === 0) return [chip(name)]
    return [index + 1 === names.length ? " and " : ", ", chip(name)]
  })
}

// The dialog's title. With the folder gone the delete is about the kept
// branches alone, so the title names them instead.
export function leftoverDeleteTitle(
  folderPresent: boolean,
  branchCount: number,
): string {
  if (folderPresent) return "Delete the left-over folder?"
  return branchCount === 1
    ? "Delete the kept branch?"
    : "Delete the kept branches?"
}

// What is deleted from disk, and why it was left there. `leftByAgent` is false
// for a folder the worktree manager's own removal left.
export function leftoverDeleteBodyProse(
  path: string,
  folderPresent: boolean,
  leftByAgent: boolean,
): Prose {
  if (!folderPresent) {
    return [
      "The left-over folder at ",
      chip(path),
      " is already gone, so nothing is deleted from disk.",
    ]
  }
  return [
    chip(path),
    ` and everything in it will be deleted from disk. Something was still writing into it when its ${leftByAgent ? "agent was deleted" : "worktree was removed"}; stop it first, or the delete fails again. This cannot be undone: dux has no trash.`,
  ]
}

// What becomes of the kept branches, following the dialog's box.
export function leftoverBranchFateProse(
  branches: readonly string[],
  willDelete: boolean,
  folderPresent: boolean,
): Prose {
  const one = branches.length === 1
  const fate = !willDelete
    ? one
      ? " is kept."
      : " are kept."
    : `${folderPresent ? " will be deleted with it" : " will be deleted"}, unless ${one ? "it has" : "they have"} moved since the folder was left behind.`
  return [one ? "The branch " : "The branches ", ...namesProse(branches), fate]
}

// Why a kept branch is not deleted, and how to delete it yourself, with the
// command as one chip.
export function leftoverKeptBranchProse(branch: UndeletableBranchView): Prose {
  const command = chip(`git branch -D ${singleQuoted(branch.name)}`)
  const lead: Prose = ["The branch ", chip(branch.name), " is kept: "]
  switch (branch.why) {
    case "no_commit":
      return [
        ...lead,
        "dux has no record of the commit it was on when the folder was left behind. Delete it yourself with ",
        command,
        ".",
      ]
    case "checked_out":
      return [
        ...lead,
        "it is checked out in ",
        chip(branch.path ?? ""),
        ". Delete it yourself with ",
        command,
        " once it is not.",
      ]
    case "moved":
      return [
        ...lead,
        "it has moved since the folder was left behind. Delete it yourself with ",
        command,
        ".",
      ]
    case "unchecked":
      return [
        ...lead,
        `git could not check it (${branch.reason ?? ""}). Delete it yourself with `,
        command,
        ".",
      ]
  }
}
