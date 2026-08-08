import { describe, expect, it } from "vitest"

import { allDeleteDiffOptions, isAllDeleteDiff } from "./diffPresentation"

// The decision behind DiffViewer's phantom-inserted-line suppression: a
// Monaco model always has at least one line, so an empty modified side draws
// one empty green "1 +" line that exists in no real content. Only a diff
// whose working side is EMPTY while HEAD has content may hide inserted-line
// decorations; every other shape has real insertions to show.
describe("isAllDeleteDiff", () => {
  it("a deleted file (HEAD content, empty modified) is an all-delete diff", () => {
    expect(
      isAllDeleteDiff({ original: "line one\nline two\n", modified: "", binary: false }),
    ).toBe(true)
  })

  it("a file truncated to zero bytes reads the same way (git reports zero added lines there too)", () => {
    expect(
      isAllDeleteDiff({ original: "content\n", modified: "", binary: false }),
    ).toBe(true)
  })

  it("an added file is not: its insertions are real", () => {
    expect(
      isAllDeleteDiff({ original: "", modified: "new\n", binary: false }),
    ).toBe(false)
  })

  it("a modified file is not: its insertions are real", () => {
    expect(
      isAllDeleteDiff({ original: "a\n", modified: "b\n", binary: false }),
    ).toBe(false)
  })

  it("empty on both sides is not (nothing to render as a deletion)", () => {
    expect(isAllDeleteDiff({ original: "", modified: "", binary: false })).toBe(
      false,
    )
  })

  it("a binary diff is never one (its sides are blanked, not empty content)", () => {
    expect(isAllDeleteDiff({ original: "", modified: "", binary: true })).toBe(
      false,
    )
    expect(
      isAllDeleteDiff({ original: "x\n", modified: "", binary: true }),
    ).toBe(false)
  })
})

// The option-level half of the suppression (Monaco cannot mount under vitest,
// so this mapping is what is honestly testable; the CSS half is proven by the
// preview-env screenshot pair).
describe("allDeleteDiffOptions", () => {
  it("an all-delete diff drops the overview ruler and the line highlight", () => {
    expect(allDeleteDiffOptions(true)).toEqual({
      renderOverviewRuler: false,
      renderLineHighlight: "none",
    })
  })

  it("every other diff keeps Monaco's defaults, stated explicitly", () => {
    expect(allDeleteDiffOptions(false)).toEqual({
      renderOverviewRuler: true,
      renderLineHighlight: "line",
    })
  })
})
