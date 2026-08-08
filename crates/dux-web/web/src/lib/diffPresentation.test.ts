import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

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

// The CSS half of the suppression is proven by the preview-env screenshots,
// but ONE selector shape is pinned here because it regressed twice, once in
// each direction: the line-number rule must scope to `.editor.modified`.
// That scope picks WHICH numbers to hide — the deleted rows' numbers are
// ordinary `.line-numbers` in the sibling `.editor.original`'s margin, so a
// wider scope hides them too (measured on ab6564e7: the red rows lost their
// 1-4) — and it doubles as the load-order guard: Monaco's own
// `.monaco-editor .margin-view-overlays .line-numbers` rule (no !important)
// ships inside the LAZY DiffViewer chunk, so it loads after index.css and a
// specificity TIE loses on source order (measured on 9c6fc2d1: the phantom
// "1" stayed). The live addStyleTag validation appends after Monaco's sheet
// and so masked the order dependence.
describe("the all-delete line-number rule's selector shape", () => {
  it("scopes to the modified editor and out-specifies Monaco's lazy-loaded rule", () => {
    const css = readFileSync(
      fileURLToPath(new URL("../index.css", import.meta.url)),
      "utf8",
    )
    expect(css).toContain(
      ".dux-diff-all-delete .editor.modified .margin-view-overlays .line-numbers",
    )
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
