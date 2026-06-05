import { describe, expect, it } from "vitest"

import { filterChangedFiles } from "./changedFiles"
import type { ChangedFileView } from "./types"

function file(path: string): ChangedFileView {
  return { status: "M", path, additions: 0, deletions: 0, binary: false }
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
