import { describe, expect, it, vi } from "vitest"

import { goToEditorSpot, readEditorSpot, type SpotEditor } from "./editorSpot"

function fakeEditor(lines: number): SpotEditor & {
  setPosition: ReturnType<typeof vi.fn>
  setScrollTop: ReturnType<typeof vi.fn>
  revealLineInCenter: ReturnType<typeof vi.fn>
} {
  return {
    getModel: () => ({ getLineCount: () => lines }),
    getPosition: () => ({ lineNumber: 30 }),
    getVisibleRanges: () => [{ startLineNumber: 21 }],
    setPosition: vi.fn(),
    setScrollTop: vi.fn(),
    revealLineInCenter: vi.fn(),
    getTopForLineNumber: (line: number) => line * 10,
  }
}

describe("the editor's spot", () => {
  it("reads the cursor line and the first visible line", () => {
    expect(readEditorSpot(fakeEditor(100))).toEqual({ line: 30, top: 21 })
  })

  it("goes to a spot past the end of a shorter file by landing on its last line", () => {
    const ed = fakeEditor(12)
    goToEditorSpot(ed, { line: 40, top: 30 })
    expect(ed.setPosition).toHaveBeenCalledWith({ lineNumber: 12, column: 1 })
    expect(ed.setScrollTop).toHaveBeenCalledWith(120)
  })

  it("centres the cursor when the address names no scroll position", () => {
    const ed = fakeEditor(100)
    goToEditorSpot(ed, { line: 40, top: null })
    expect(ed.revealLineInCenter).toHaveBeenCalledWith(40)
    expect(ed.setScrollTop).not.toHaveBeenCalled()
  })
})
