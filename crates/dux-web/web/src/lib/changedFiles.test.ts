import { describe, expect, it } from "vitest"

import {
  editableFiles,
  filterChangedFiles,
  shouldShowChangedFiles,
  statusGlyph,
} from "./changedFiles"
import type { ChangedFileView } from "./types"

function file(path: string, status = "M"): ChangedFileView {
  return { status, path, additions: 0, deletions: 0, binary: false }
}

const files = [
  file("src/app/main.rs"),
  file("src/lib/Store.ts"),
  file("README.md"),
]

describe("filterChangedFiles", () => {
  it("matches a case-insensitive substring on the path", () => {
    const result = filterChangedFiles(files, "store")
    expect(result.map((f) => f.path)).toEqual(["src/lib/Store.ts"])
  })

  it("matches across path segments", () => {
    const result = filterChangedFiles(files, "src/")
    expect(result.map((f) => f.path)).toEqual([
      "src/app/main.rs",
      "src/lib/Store.ts",
    ])
  })

  it("returns nothing when no path matches", () => {
    expect(filterChangedFiles(files, "nope")).toEqual([])
  })

  it("passes everything through for an empty query", () => {
    expect(filterChangedFiles(files, "")).toEqual(files)
  })

  it("passes everything through for a whitespace-only query", () => {
    expect(filterChangedFiles(files, "   ")).toEqual(files)
  })
})

describe("shouldShowChangedFiles", () => {
  it("shows when the watched session matches the selection", () => {
    expect(shouldShowChangedFiles("s1", "s1")).toBe(true)
  })

  it("hides when the watch belongs to a different session", () => {
    expect(shouldShowChangedFiles("s2", "s1")).toBe(false)
  })

  it("hides while the server hasn't started watching yet", () => {
    expect(shouldShowChangedFiles(null, "s1")).toBe(false)
  })

  it("hides when nothing is selected", () => {
    expect(shouldShowChangedFiles("s1", null)).toBe(false)
    expect(shouldShowChangedFiles(null, null)).toBe(false)
  })
})

describe("statusGlyph", () => {
  it("maps known codes to a single glyph", () => {
    expect(statusGlyph("M")).toBe("M")
    expect(statusGlyph("a")).toBe("A")
    expect(statusGlyph("??")).toBe("?")
    expect(statusGlyph("?")).toBe("?")
  })

  it("falls back to the first uppercased char for unknown codes", () => {
    expect(statusGlyph("rm")).toBe("R")
    expect(statusGlyph("")).toBe("?")
  })
})

describe("editableFiles", () => {
  it("merges staged and unstaged, deduping by path", () => {
    const staged = [file("a.ts"), file("shared.ts")]
    const unstaged = [file("shared.ts"), file("b.ts")]
    const result = editableFiles(staged, unstaged).map((f) => f.path)
    expect(result).toEqual(["shared.ts", "b.ts", "a.ts"])
  })

  it("drops deleted files (nothing on disk to edit)", () => {
    const staged = [file("gone.ts", "D")]
    const unstaged = [file("kept.ts"), file("also-gone.ts", "d")]
    expect(editableFiles(staged, unstaged).map((f) => f.path)).toEqual([
      "kept.ts",
    ])
  })

  it("keeps the unstaged status when a path is in both lists", () => {
    const staged = [file("x.ts", "A")]
    const unstaged = [file("x.ts", "M")]
    expect(editableFiles(staged, unstaged)).toEqual([
      { status: "M", path: "x.ts", additions: 0, deletions: 0, binary: false },
    ])
  })
})
