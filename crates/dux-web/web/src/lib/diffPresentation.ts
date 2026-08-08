// Presentation decisions about a fetched diff payload (lib/fileApi's
// FileDiffContents shape), kept pure so they are unit-testable without
// mounting Monaco (which cannot run under vitest, see monacoSetup.ts).

// Whether a diff renders as an ALL-DELETE diff: HEAD has content and the
// working side is empty. DiffViewer uses this to suppress Monaco's phantom
// trailing "1 +" inserted line on such diffs: a Monaco text model always has
// at least one line, so an empty modified side is one empty line, and the
// diff computer reports a real insertion for it (measured on monaco 0.55.1:
// original [1,N) -> modified [1,2) for every original shape, trailing newline
// or not). That added line exists in no real content — git reports zero added
// lines for a deletion — so hiding its decorations is honest, and the
// original side's text is never touched.
//
// Content-based on purpose, not git-status-based: a file truncated to zero
// bytes (status M) produces the same empty modified side, and git equally
// reports zero added lines for it, so the phantom is just as false there.
// The binary arm renders its own refusal before DiffViewer mounts; excluding
// it here keeps the answer honest for callers that ask earlier.
export function isAllDeleteDiff(diff: {
  original: string
  modified: string
  binary: boolean
}): boolean {
  return !diff.binary && diff.original !== "" && diff.modified === ""
}

// The Monaco diff-editor options that vary with the all-delete decision.
// Extracted (rather than branched inline in DiffViewer) because Monaco cannot
// mount under vitest, so this mapping is the only honestly testable half of
// the suppression; the CSS half is validated by screenshot in the preview
// env. On an all-delete diff:
//  - the overview ruler is a CANVAS, so the phantom insertion's green speck
//    on it is unreachable by CSS; dropping the ruler is the only way to kill
//    it, and an all-delete ruler said nothing anyway (every line is red);
//  - the current-line highlight is what draws a border around the phantom
//    empty row the CSS rules blank out (the TextModel really has one line),
//    so it goes too.
// Every other diff keeps Monaco's defaults, stated explicitly so a future
// Monaco default change cannot silently flip them.
export function allDeleteDiffOptions(allDelete: boolean): {
  renderOverviewRuler: boolean
  renderLineHighlight: "line" | "none"
} {
  return allDelete
    ? { renderOverviewRuler: false, renderLineHighlight: "none" }
    : { renderOverviewRuler: true, renderLineHighlight: "line" }
}
