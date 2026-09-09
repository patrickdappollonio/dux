// Presentation decisions about a fetched diff payload, kept pure so they are
// testable without mounting Monaco, which cannot run under vitest.

// Whether a diff renders as an all-delete diff: HEAD has content and the working
// side is empty. A Monaco text model always holds at least one line, so an empty
// modified side reports a phantom inserted line that exists in no real content,
// and DiffViewer suppresses its decorations. Content-based rather than
// git-status-based, since a file truncated to zero bytes has the same phantom.
export function isAllDeleteDiff(diff: {
  original: string
  modified: string
  binary: boolean
}): boolean {
  return !diff.binary && diff.original !== "" && diff.modified === ""
}

// The Monaco diff-editor options that vary with the all-delete decision. On an
// all-delete diff the overview ruler goes, because it is a canvas and the
// phantom insertion's speck on it is unreachable by CSS, and the current-line
// highlight goes, because it borders the phantom row the CSS blanks out. Every
// other diff keeps Monaco's defaults, stated explicitly so a future default
// change cannot silently flip them.
export function allDeleteDiffOptions(allDelete: boolean): {
  renderOverviewRuler: boolean
  renderLineHighlight: "line" | "none"
} {
  return allDelete
    ? { renderOverviewRuler: false, renderLineHighlight: "none" }
    : { renderOverviewRuler: true, renderLineHighlight: "line" }
}
