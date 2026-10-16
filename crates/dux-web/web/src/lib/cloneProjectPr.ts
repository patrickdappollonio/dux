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
  // Whether the agent will need a review copy, for the folder as it stands: the
  // offer's answer, or the folder check's once it has looked at a checkout.
  reviewCopy: ReviewCopyOutlook
  reviewCopyAgreed: boolean
}

// The review-copy question applies only while the agent is named after the
// pull request's branch. Any other name is its own branch, and the server
// checks that one itself.
export function reviewCopyShown(pr: CloneProjectPr, name: string): ReviewCopyOutlook {
  return name.trim() === pr.offer.head_branch ? pr.reviewCopy : { state: "not_needed" }
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
// Null when the answer is what is already on screen.
export function foldFolderAnswer(
  pr: CloneProjectPr,
  answer: { state: "free" | "checkout"; review_copy?: ReviewCopyOutlook },
): CloneProjectPr | null {
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
