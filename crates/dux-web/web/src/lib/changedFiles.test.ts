import { describe, expect, it } from "vitest"

import {
  fileStatusMeta,
  filterChangedFiles,
  formatRecapCount,
  mergeChangedFilesRecaps,
  reconcileSelection,
  shouldShowChangedFiles,
  summarizeChangedFiles,
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

describe("reconcileSelection", () => {
  const slice = {
    staged: [file("kept-staged.ts")],
    unstaged: [file("kept-unstaged.ts"), file("moved.ts")],
  }

  it("keeps a path that is still in its own section", () => {
    const next = reconcileSelection(
      { staged: new Set(["kept-staged.ts"]), unstaged: new Set(["kept-unstaged.ts"]) },
      slice,
    )
    expect([...next.staged]).toEqual(["kept-staged.ts"])
    expect([...next.unstaged]).toEqual(["kept-unstaged.ts"])
  })

  it("drops a path that vanished from the changes entirely", () => {
    const next = reconcileSelection(
      { staged: new Set(["gone.ts"]), unstaged: new Set(["also-gone.ts"]) },
      slice,
    )
    expect(next.staged.size).toBe(0)
    expect(next.unstaged.size).toBe(0)
  })

  // A file selected to be staged, then staged: it is no longer "selected to
  // stage", so it leaves the unstaged set rather than following the file.
  it("drops a path that moved to the other section", () => {
    const next = reconcileSelection(
      { staged: new Set(["moved.ts"]), unstaged: new Set(["kept-staged.ts"]) },
      slice,
    )
    expect(next.staged.size).toBe(0)
    expect(next.unstaged.size).toBe(0)
  })
})

function counted(
  path: string,
  additions: number,
  deletions: number,
  binary = false,
): ChangedFileView {
  return { status: "M", path, additions, deletions, binary }
}

describe("summarizeChangedFiles", () => {
  it("adds the lines up across the files it is given", () => {
    expect(
      summarizeChangedFiles([
        counted("a.ts", 12, 3),
        counted("b.ts", 7, 40),
      ]),
    ).toEqual({ count: 2, additions: 19, deletions: 43, binaryCount: 0 })
  })

  // Binary files carry no line counts on the wire, so they must be counted
  // apart rather than folded into the sums as zeroes.
  it("counts binary files apart and takes no lines from them", () => {
    expect(
      summarizeChangedFiles([
        counted("a.ts", 5, 1),
        counted("logo.png", 0, 0, true),
        counted("clip.mp4", 0, 0, true),
      ]),
    ).toEqual({ count: 3, additions: 5, deletions: 1, binaryCount: 2 })
  })

  it("reports an all-binary set as lineless", () => {
    expect(summarizeChangedFiles([counted("logo.png", 0, 0, true)])).toEqual({
      count: 1,
      additions: 0,
      deletions: 0,
      binaryCount: 1,
    })
  })

  it("reports an empty set as all zeroes", () => {
    expect(summarizeChangedFiles([])).toEqual({
      count: 0,
      additions: 0,
      deletions: 0,
      binaryCount: 0,
    })
  })

  // The recap describes exactly the rows visible beneath it, so a caller hands
  // it the filtered list and gets the filtered figures.
  it("describes only the files handed to it, filtering included", () => {
    const all = [counted("src/a.ts", 10, 0), counted("docs/b.md", 100, 5)]
    expect(summarizeChangedFiles(filterChangedFiles(all, "src/"))).toEqual({
      count: 1,
      additions: 10,
      deletions: 0,
      binaryCount: 0,
    })
  })
})

// The TUI's `format_recap_count` answers these very cases identically; the two
// helpers are kept in step by hand, so a change here belongs in both suites.
describe("formatRecapCount", () => {
  it("prints anything under a thousand as it is", () => {
    expect(formatRecapCount(0)).toBe("0")
    expect(formatRecapCount(999)).toBe("999")
  })

  it("reads in thousands from a thousand up, dropping a zero decimal", () => {
    expect(formatRecapCount(1000)).toBe("1k")
    expect(formatRecapCount(1300)).toBe("1.3k")
    expect(formatRecapCount(10000)).toBe("10k")
    expect(formatRecapCount(12345)).toBe("12.3k")
  })

  // Truncated, never rounded: the figure must not claim more lines than there
  // are, so 1050 stays "1k" and 1999 stays "1.9k".
  it("truncates the decimal rather than rounding it up", () => {
    expect(formatRecapCount(1049)).toBe("1k")
    expect(formatRecapCount(1050)).toBe("1k")
    expect(formatRecapCount(1999)).toBe("1.9k")
    expect(formatRecapCount(9999)).toBe("9.9k")
  })
})

describe("mergeChangedFilesRecaps", () => {
  it("adds two recaps field by field", () => {
    expect(
      mergeChangedFilesRecaps(
        { count: 2, additions: 5, deletions: 1, binaryCount: 0 },
        { count: 3, additions: 4, deletions: 9, binaryCount: 2 },
      ),
    ).toEqual({ count: 5, additions: 9, deletions: 10, binaryCount: 2 })
  })
})
