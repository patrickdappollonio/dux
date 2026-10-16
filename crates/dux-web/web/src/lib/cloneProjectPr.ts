import type { PullRequestCloneOffer, ReviewCopyOutlook } from "./sessionsApi"

// What the clone dialog knows when it is offering a pull request's repository
// rather than a plain clone.
export interface CloneProjectPr {
  // The text typed into the reference field, kept so choosing an existing
  // project (or a question found after the clone) can put it back.
  reference: string
  // `host/owner/repo`, or `owner/repo` when the reference named no host.
  repository: string
  // The clause naming projects dux could not check, when some were skipped.
  incompleteSummary: string | null
  hasProjects: boolean
  offer: PullRequestCloneOffer
  // What the folder holds as the dialog last saw it: a clone, or an add.
  folder: "free" | "checkout"
  // Whether the agent will need a review copy when it is named after the pull
  // request's branch, for the folder as it stands: the offer's answer, or the
  // folder check's once it has looked at a checkout.
  reviewCopy: ReviewCopyOutlook
  // The answer for a name typed in the form of a review copy of the branch,
  // which is the server's to give: only that name's.
  typed: { name: string; outlook: ReviewCopyOutlook } | null
  reviewCopyAgreed: boolean
}

const NOT_NEEDED: ReviewCopyOutlook = { state: "not_needed" }

// The review-copy question for the name as it stands: the branch's own answer
// for the branch's own name, the typed name's answer for the name it was asked
// about, and none for any other name (the server checks that one itself).
export function reviewCopyShown(pr: CloneProjectPr, name: string): ReviewCopyOutlook {
  const trimmed = name.trim()
  if (trimmed === pr.offer.head_branch) return pr.reviewCopy
  return pr.typed?.name === trimmed ? pr.typed.outlook : NOT_NEEDED
}

// Whether the name is spelled like a review copy of the branch (`-review`, or
// `-review-` and a number), the one kind of typed name that can need the
// question. The server decides; this only says whether to ask it.
export function looksLikeReviewCopy(pr: CloneProjectPr, name: string): boolean {
  const trimmed = name.trim()
  if (!trimmed.startsWith(pr.offer.head_branch)) return false
  return /^-review(-\d+)?$/.test(trimmed.slice(pr.offer.head_branch.length))
}

// Whether the button must stay disabled for want of an answer or a free name.
export function reviewCopyBlocksSubmit(pr: CloneProjectPr, name: string): boolean {
  const shown = reviewCopyShown(pr, name)
  return (
    shown.state === "no_free_name" ||
    (shown.state === "needed" && !pr.reviewCopyAgreed)
  )
}

// The agreement a submit carries: exactly what the dialog showed and the user
// ticked, or nothing.
export function reviewCopyAgreement(
  pr: CloneProjectPr,
  name: string,
): { review_copy: true; review_copy_reason: string; review_copy_branch: string } | null {
  const shown = reviewCopyShown(pr, name)
  if (shown.state !== "needed" || !pr.reviewCopyAgreed) return null
  return {
    review_copy: true,
    review_copy_reason: shown.reason,
    review_copy_branch: shown.copy_branch,
  }
}

// A folder check that answers differently from what the dialog shows changes
// the offer (clone or add) or the review-copy question. The caller switches and
// waits for another press, so the user agrees to what will actually happen.
// Null when the answer is what is already on screen. `name` is the name the
// check was asked about: a typed name's answer is that name's, not the branch's.
export function foldFolderAnswer(
  pr: CloneProjectPr,
  answer: { state: "free" | "checkout"; review_copy?: ReviewCopyOutlook },
  name: string,
): CloneProjectPr | null {
  const trimmed = name.trim()
  if (trimmed !== "" && trimmed !== pr.offer.head_branch) {
    const outlook: ReviewCopyOutlook = answer.review_copy ?? NOT_NEEDED
    const shown = pr.typed?.name === trimmed ? pr.typed.outlook : NOT_NEEDED
    if (answer.state === pr.folder && JSON.stringify(outlook) === JSON.stringify(shown)) {
      return null
    }
    return {
      ...pr,
      folder: answer.state,
      // The branch's own answer is no longer known for a different folder.
      reviewCopy:
        answer.state === pr.folder
          ? pr.reviewCopy
          : answer.state === "checkout"
            ? { state: "unknown" }
            : pr.offer.review_copy,
      typed: { name: trimmed, outlook },
      reviewCopyAgreed: false,
    }
  }
  const reviewCopy: ReviewCopyOutlook =
    answer.state === "checkout"
      ? (answer.review_copy ?? { state: "unknown" })
      : pr.offer.review_copy
  if (
    answer.state === pr.folder &&
    JSON.stringify(reviewCopy) === JSON.stringify(pr.reviewCopy)
  ) {
    return null
  }
  return { ...pr, folder: answer.state, reviewCopy, reviewCopyAgreed: false }
}
