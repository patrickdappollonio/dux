// What the worktree manager says after removing one worktree.
//
// The report is derived from the SERVER'S ANSWER, never from the checkbox the
// request carried. Those are different questions: `git branch -D` refuses a
// branch that is still checked out somewhere, so "the user asked for the
// branch" and "the branch is gone" come apart, and the toast used to assert
// the second while only knowing the first.
//
// Pure, so every rung is testable without a server or a toast.

import type { FinalTone } from "@/lib/notify"

/// What the server says happened to the branch. Mirrors
/// `project_reads.rs`'s `BranchOutcomeReply`; `null`/absent means no branch
/// deletion was attempted, so nothing may be claimed about one.
export interface WorktreeBranchOutcome {
  name: string
  outcome: "deleted" | "already_gone" | "refused" | (string & {})
  reason?: string | null
}

export interface DeleteWorktreeReply {
  branch?: WorktreeBranchOutcome | null
}

export interface WorktreeDeleteReport {
  tone: FinalTone
  message: string
  /// See the toast tenet: sticky only when the user must act OUTSIDE the toast
  /// to recover, or something was left half-done. A refused branch is both, so
  /// it is the one rung here that pins.
  sticky: boolean
}

/// The toast for a successful worktree removal.
export function worktreeDeleteReport(
  worktreePath: string,
  reply: DeleteWorktreeReply | null | undefined,
): WorktreeDeleteReport {
  const branch = reply?.branch ?? null
  if (branch === null) {
    return {
      tone: "success",
      message: `Removed the worktree at ${worktreePath}. Its branch is still there.`,
      sticky: false,
    }
  }
  if (branch.outcome === "deleted") {
    return {
      tone: "success",
      message: `Removed the worktree at ${worktreePath} and deleted its branch "${branch.name}".`,
      sticky: false,
    }
  }
  if (branch.outcome === "already_gone") {
    return {
      tone: "success",
      message: `Removed the worktree at ${worktreePath}. Its branch "${branch.name}" was already gone.`,
      sticky: false,
    }
  }
  // Refused, and anything the server may add later: the branch is still there,
  // so say so, quote git, and name the way out. Falling through to the
  // success wording would be the lie this whole path exists to remove.
  const reason = cleanReason(branch.reason)
  return {
    tone: "warning",
    message:
      `Removed the worktree at ${worktreePath}, but git refused to delete its branch ` +
      `"${branch.name}": ${reason} Delete it yourself with git branch -D "${branch.name}", ` +
      `or leave it and give the next agent a different name.`,
    sticky: true,
  }
}

/// git's stderr line, tidied the way `dux_core::git`'s own note does it: the
/// "error: " prefix dropped and a full stop added when git did not end with
/// one, so the sentence around it reads.
function cleanReason(reason: string | null | undefined): string {
  const trimmed = (reason ?? "").trim()
  const stripped = trimmed.startsWith("error: ")
    ? trimmed.slice("error: ".length)
    : trimmed.startsWith("fatal: ")
      ? trimmed.slice("fatal: ".length)
      : trimmed
  if (stripped === "") return "git gave no reason."
  return /[.!?]$/.test(stripped) ? stripped : `${stripped}.`
}
