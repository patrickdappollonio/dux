// Report branch deletion from the server outcome, not the requested checkbox:
// git may refuse to delete a branch still checked out elsewhere.

import type { FinalTone } from "@/lib/notify"
import { chip, type Prose, prose, quotedChip, wireProse } from "@/lib/prose"
import { singleQuoted } from "@/lib/shellQuote"
import type { DeleteLeftoverFolderReply } from "@/lib/types"

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
  /// Every name (the path, the branch, the command to run) is a chip.
  message: Prose
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
      message: prose`Removed the worktree at ${chip(worktreePath)}. Its branch is still there.`,
      sticky: false,
    }
  }
  if (branch.outcome === "deleted") {
    return {
      tone: "success",
      message: prose`Removed the worktree at ${chip(worktreePath)} and deleted its branch ${quotedChip(branch.name)}.`,
      sticky: false,
    }
  }
  if (branch.outcome === "already_gone") {
    return {
      tone: "success",
      message: prose`Removed the worktree at ${chip(worktreePath)}. Its branch ${quotedChip(branch.name)} was already gone.`,
      sticky: false,
    }
  }
  // Refused, and anything the server may add later: the branch is still there,
  // so say so, quote git, and name the way out. Falling through to the
  // success wording would be the lie this whole path exists to remove.
  const reason = cleanReason(branch.reason)
  return {
    tone: "warning",
    message: prose`Removed the worktree at ${chip(worktreePath)}, but git refused to delete its branch ${quotedChip(branch.name)}: ${reason} Delete it yourself with ${chip(`git branch -D ${singleQuoted(branch.name)}`)}, or leave it and give the next agent a different name.`,
    sticky: true,
  }
}

/// The toast for a left-over folder delete, from the server's answer. The
/// sentence is dux-core's, shared with the terminal UI, so nothing is reworded
/// here, and the tone is the server's `warning` flag rather than `removed`:
/// every refusal is a warning, and so is a branch that moved, refused or could
/// not be checked, while a folder that was already gone is not removed by this
/// request and is no warning by itself. No outcome is sticky, a warning
/// included (the user's decision): each one leaves on its tone's normal
/// window. A left-over record lives as long as its folder or a kept branch
/// remains, so a delete that leaves either one, a branch it kept included,
/// leaves the worktree manager's row in place showing it.
export function leftoverDeleteReport(
  reply: DeleteLeftoverFolderReply,
): { tone: FinalTone; message: string | Prose } {
  return {
    tone: reply.warning ? "warning" : "success",
    message: wireProse(reply.message, reply.segments),
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
