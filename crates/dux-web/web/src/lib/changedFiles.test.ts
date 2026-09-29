import { describe, expect, it } from "vitest"

import {
  fileStatusMeta,
  filterChangedFiles,
  formatRecapCount,
  mergeChangedFilesRecaps,
  CHANGED_FILE_FIELDS,
  changedFileCount,
  folderCountLabel,
  reconcileSelection,
  reuseUnchangedFiles,
  summarizeChangedFiles,
} from "./changedFiles"
import type { ChangedFileView } from "./types"

function file(path: string, status = "M"): ChangedFileView {
  return {
    status,
    path,
    additions: 0,
    deletions: 0,
    binary: false,
    diff_excluded: false,
  }
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

  // A caller memoizes on the selection's identity, so "nothing dropped" must be
  // answered by reference rather than by a rebuilt copy of every set.
  it("returns the same selection when every checked path survives", () => {
    const prev = {
      staged: new Set(["kept-staged.ts"]),
      unstaged: new Set(["kept-unstaged.ts"]),
    }
    expect(reconcileSelection(prev, slice)).toBe(prev)
  })

  it("keeps the surviving section's set when only the other one drops a path", () => {
    const prev = {
      staged: new Set(["kept-staged.ts"]),
      unstaged: new Set(["gone.ts"]),
    }
    const next = reconcileSelection(prev, slice)
    expect(next).not.toBe(prev)
    expect(next.staged).toBe(prev.staged)
    expect(next.unstaged.size).toBe(0)
  })

  it("does not index the live list for an empty selection", () => {
    const huge = {
      staged: [] as ChangedFileView[],
      unstaged: new Proxy([] as ChangedFileView[], {
        get(target, key, receiver) {
          if (key === "map") throw new Error("indexed the list for nothing")
          return Reflect.get(target, key, receiver)
        },
      }),
    }
    const prev = { staged: new Set<string>(), unstaged: new Set<string>() }
    expect(reconcileSelection(prev, huge)).toBe(prev)
  })
})

describe("reuseUnchangedFiles", () => {
  it("returns the previous array when the fresh one says the same thing", () => {
    const previous = [file("a.ts"), file("b.ts", "??")]
    const next = [file("a.ts"), file("b.ts", "??")]
    expect(reuseUnchangedFiles(previous, next)).toBe(previous)
  })

  it("keeps each unchanged file's object and takes the changed ones", () => {
    const previous = [file("a.ts"), file("b.ts")]
    const moved = { ...file("b.ts"), additions: 4 }
    const result = reuseUnchangedFiles(previous, [file("a.ts"), moved, file("c.ts")])
    expect(result).not.toBe(previous)
    expect(result[0]).toBe(previous[0])
    expect(result[1]).toBe(moved)
    expect(result.map((f) => f.path)).toEqual(["a.ts", "b.ts", "c.ts"])
  })

  it("treats a rename's source as part of the file", () => {
    const previous = [{ ...file("new.ts", "R"), renamed_from: "old.ts" }]
    const next = [{ ...file("new.ts", "R"), renamed_from: "other.ts" }]
    expect(reuseUnchangedFiles(previous, next)[0]).toBe(next[0])
  })

  // `Required<ChangedFileView>` stops compiling when the wire grows a field
  // this sample does not set, and the comparator's field list is typed against
  // the same keys, so a new field cannot slip past the comparison unseen.
  it("compares every field of the wire type", () => {
    const full: Required<ChangedFileView> = {
      status: "R",
      path: "new.ts",
      additions: 3,
      deletions: 2,
      binary: false,
      diff_excluded: false,
      renamed_from: "old.ts",
      kind: "directory",
      file_count: 3,
      nested_repositories: 0,
      fingerprint: "aa",
    }
    expect(Object.keys(CHANGED_FILE_FIELDS).sort()).toEqual(Object.keys(full).sort())
    const changed: { [K in keyof ChangedFileView]-?: ChangedFileView[K] } = {
      status: "M",
      path: "other.ts",
      additions: 4,
      deletions: 5,
      binary: true,
      diff_excluded: true,
      renamed_from: "elsewhere.ts",
      kind: "nested_repository",
      file_count: 4,
      nested_repositories: 1,
      fingerprint: "bb",
    }
    for (const key of Object.keys(full) as (keyof ChangedFileView)[]) {
      const next = { ...full, [key]: changed[key] }
      expect(reuseUnchangedFiles([full], [next])[0], key).toBe(next)
    }
    expect(reuseUnchangedFiles([full], [{ ...full }])[0]).toBe(full)
  })

  it("follows the fresh order when files only moved position", () => {
    const previous = [file("a.ts"), file("b.ts")]
    const result = reuseUnchangedFiles(previous, [file("b.ts"), file("a.ts")])
    expect(result).toEqual([previous[1], previous[0]])
  })
})

function counted(
  path: string,
  additions: number,
  deletions: number,
  binary = false,
  diffExcluded = false,
): ChangedFileView {
  return {
    status: "M",
    path,
    additions,
    deletions,
    binary,
    diff_excluded: diffExcluded,
  }
}

describe("summarizeChangedFiles", () => {
  it("adds the lines up across the files it is given", () => {
    expect(
      summarizeChangedFiles([
        counted("a.ts", 12, 3),
        counted("b.ts", 7, 40),
      ]),
    ).toEqual({ count: 2, additions: 19, deletions: 43, binaryCount: 0, diffExcludedCount: 0 })
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
    ).toEqual({ count: 3, additions: 5, deletions: 1, binaryCount: 2, diffExcludedCount: 0 })
  })

  it("reports an all-binary set as lineless", () => {
    expect(summarizeChangedFiles([counted("logo.png", 0, 0, true)])).toEqual({
      count: 1,
      additions: 0,
      deletions: 0,
      binaryCount: 1,
      diffExcludedCount: 0,
    })
  })

  it("reports an empty set as all zeroes", () => {
    expect(summarizeChangedFiles([])).toEqual({
      count: 0,
      additions: 0,
      deletions: 0,
      binaryCount: 0,
      diffExcludedCount: 0,
    })
  })

  // A file the repository excludes from diffs has no line counts either, and it
  // is not binary: it is tallied on its own so the two are never confused.
  it("counts diff-excluded files apart from the binaries", () => {
    expect(
      summarizeChangedFiles([
        counted("a.ts", 5, 1),
        counted("logo.png", 0, 0, true),
        counted("locked.txt", 0, 0, false, true),
        counted("also-locked.txt", 0, 0, false, true),
      ]),
    ).toEqual({
      count: 4,
      additions: 5,
      deletions: 1,
      binaryCount: 1,
      diffExcludedCount: 2,
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
      diffExcludedCount: 0,
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
        {
          count: 2,
          additions: 5,
          deletions: 1,
          binaryCount: 0,
          diffExcludedCount: 1,
        },
        {
          count: 3,
          additions: 4,
          deletions: 9,
          binaryCount: 2,
          diffExcludedCount: 3,
        },
      ),
    ).toEqual({
      count: 5,
      additions: 9,
      deletions: 10,
      binaryCount: 2,
      diffExcludedCount: 4,
    })
  })
})

describe("folded folders in the totals", () => {
  it("counts a folder by the files inside it", () => {
    const folder: ChangedFileView = {
      ...file("node_modules", "??"),
      kind: "directory",
      file_count: 28747,
    }
    const nested: ChangedFileView = {
      ...file("vendor/lib", "??"),
      kind: "nested_repository",
    }
    expect(changedFileCount(folder)).toBe(28747)
    expect(changedFileCount(nested)).toBe(1)
    expect(changedFileCount(file("a.ts"))).toBe(1)
    expect(summarizeChangedFiles([folder, nested, file("a.ts")]).count).toBe(28749)
  })

  it("names the nested repositories a folder holds apart from its files", () => {
    const withNested = (files: number, nested: number): ChangedFileView => ({
      ...file("vendor", "??"),
      kind: "directory",
      file_count: files,
      nested_repositories: nested,
    })
    expect(folderCountLabel(withNested(3, 1))).toBe("3 files and 1 nested repository")
    expect(folderCountLabel(withNested(0, 2))).toBe("2 nested repositories")
    expect(folderCountLabel(withNested(28747, 0))).toBe("28,747 files")
  })

  it("compares the folder fields like every other field", () => {
    const before: ChangedFileView = { ...file("dist", "??"), kind: "directory", file_count: 3 }
    const after: ChangedFileView = { ...before, file_count: 4 }
    expect(reuseUnchangedFiles([before], [after])).toEqual([after])
    expect(reuseUnchangedFiles([before], [after])[0]).toBe(after)
  })
})
