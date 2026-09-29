import { describe, expect, it } from "vitest"

import { discardOutcome, selectedFileCount } from "./discardOutcome"
import { proseText } from "./prose"
import type { ChangedFileView } from "./types"

function row(path: string, status: string, extra: Partial<ChangedFileView> = {}) {
  return {
    path,
    status,
    additions: 0,
    deletions: 0,
    binary: false,
    diff_excluded: false,
    ...extra,
  } as ChangedFileView
}

const folder = row("node_modules", "??", { kind: "directory", file_count: 28747 })
const nested = row("vendor/lib", "??", { kind: "nested_repository" })
const untracked = row("notes.md", "??")
const tracked = row("src/main.rs", "M")

describe("discardOutcome", () => {
  it("names a folder with its file count and what it kept", () => {
    expect(proseText(discardOutcome([folder]))).toBe(
      "Deleted the untracked files in node_modules/ (28,747 files); files the repository " +
        "ignores and repositories of their own inside it are kept.",
    )
  })

  it("says a repository of its own went with its history", () => {
    expect(proseText(discardOutcome([nested]))).toBe(
      "Deleted vendor/lib/, a repository of its own, with its history.",
    )
  })

  it("keeps the file wording for files", () => {
    expect(proseText(discardOutcome([untracked]))).toBe(
      "Deleted the untracked file notes.md.",
    )
    expect(proseText(discardOutcome([tracked]))).toBe(
      "Discarded the unstaged changes to src/main.rs. Staged changes, if any, are kept.",
    )
  })

  it("counts files, not rows, for a batch", () => {
    expect(proseText(discardOutcome([folder, untracked, nested]))).toBe(
      "Discarded the changes to 28,748 files, and deleted 1 repository of its own with " +
        "its history. Files the repository ignores and repositories of their own inside " +
        "the folders are kept.",
    )
  })

  it("chips the folder it names", () => {
    const names = discardOutcome([folder]).filter((segment) => typeof segment !== "string")
    expect(names).toEqual([{ name: "node_modules/", quoted: false }])
  })
})

describe("selectedFileCount", () => {
  it("counts the files inside a selected folder", () => {
    expect(
      selectedFileCount(new Set(["node_modules", "notes.md"]), [folder, untracked, tracked]),
    ).toBe(28748)
  })
})
