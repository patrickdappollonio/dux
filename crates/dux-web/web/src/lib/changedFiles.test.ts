import { describe, expect, it } from "vitest"

import {
  fileStatusMeta,
  filterChangedFiles,
  shouldShowChangedFiles,
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

describe("fileStatusMeta", () => {
  it("maps known codes to a kind and label", () => {
    expect(fileStatusMeta("M")).toEqual({ kind: "modified", label: "Modified" })
    expect(fileStatusMeta("a")).toEqual({ kind: "added", label: "Added" })
    expect(fileStatusMeta("D")).toEqual({ kind: "deleted", label: "Deleted" })
    expect(fileStatusMeta("R")).toEqual({ kind: "renamed", label: "Renamed" })
    expect(fileStatusMeta("??")).toEqual({
      kind: "untracked",
      label: "Untracked",
    })
    expect(fileStatusMeta("?")).toEqual({
      kind: "untracked",
      label: "Untracked",
    })
  })

  it("keys off the first significant char for multi-char codes", () => {
    expect(fileStatusMeta("rm")).toEqual({ kind: "renamed", label: "Renamed" })
    expect(fileStatusMeta("MM")).toEqual({ kind: "modified", label: "Modified" })
    expect(fileStatusMeta("R ")).toEqual({ kind: "renamed", label: "Renamed" })
  })

  it("maps copy, conflict, and type-change codes", () => {
    expect(fileStatusMeta("C")).toEqual({ kind: "copied", label: "Copied" })
    expect(fileStatusMeta("U")).toEqual({ kind: "conflict", label: "Conflict" })
    expect(fileStatusMeta("UU")).toEqual({ kind: "conflict", label: "Conflict" })
    expect(fileStatusMeta("T")).toEqual({
      kind: "type-changed",
      label: "Type changed",
    })
  })

  it("falls back to a generic 'Changed' label for unknown or empty codes", () => {
    expect(fileStatusMeta("")).toEqual({ kind: "other", label: "Changed" })
    expect(fileStatusMeta("X")).toEqual({ kind: "other", label: "Changed" })
  })
})
