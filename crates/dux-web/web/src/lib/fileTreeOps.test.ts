import { describe, expect, it } from "vitest"

import {
  joinName,
  parentDir,
  renameTarget,
  targetDirForCreate,
  validateEntryName,
} from "@/lib/fileTreeOps"

describe("targetDirForCreate", () => {
  it("targets the parent dir for a file row", () => {
    expect(targetDirForCreate({ kind: "file", path: "src/a/b.ts" })).toBe("src/a")
  })

  it("targets the parent dir (root) for a top-level file row", () => {
    expect(targetDirForCreate({ kind: "file", path: "b.ts" })).toBe("")
  })

  it("targets the folder itself for a dir row", () => {
    expect(targetDirForCreate({ kind: "dir", path: "src/a" })).toBe("src/a")
  })

  it("targets the worktree root for the root/empty-area context", () => {
    expect(targetDirForCreate({ kind: "root" })).toBe("")
  })
})

describe("parentDir", () => {
  it("returns the parent of a nested path", () => {
    expect(parentDir("a/b/c.ts")).toBe("a/b")
  })

  it("returns '' for a top-level path", () => {
    expect(parentDir("x")).toBe("")
  })

  it("returns '' for ''", () => {
    expect(parentDir("")).toBe("")
  })
})

describe("joinName", () => {
  it("returns just the name when dir is the root", () => {
    expect(joinName("", "foo.ts")).toBe("foo.ts")
  })

  it("joins dir and name with a slash", () => {
    expect(joinName("src/a", "foo.ts")).toBe("src/a/foo.ts")
  })
})

describe("validateEntryName", () => {
  it("rejects an empty name", () => {
    expect(validateEntryName("").ok).toBe(false)
  })

  it("rejects a whitespace-only name", () => {
    expect(validateEntryName("   ").ok).toBe(false)
  })

  it("rejects a name containing a forward slash", () => {
    expect(validateEntryName("a/b").ok).toBe(false)
  })

  it("rejects a name containing a backslash", () => {
    expect(validateEntryName("a\\b").ok).toBe(false)
  })

  it("rejects '.'", () => {
    expect(validateEntryName(".").ok).toBe(false)
  })

  it("rejects '..'", () => {
    expect(validateEntryName("..").ok).toBe(false)
  })

  it("rejects '.git' case-insensitively", () => {
    expect(validateEntryName(".git").ok).toBe(false)
    expect(validateEntryName(".GIT").ok).toBe(false)
    expect(validateEntryName(".Git").ok).toBe(false)
  })

  it("rejects a name containing a NUL byte", () => {
    expect(validateEntryName("a\0b").ok).toBe(false)
  })

  it("rejects a name containing a control character", () => {
    expect(validateEntryName("a\tb").ok).toBe(false)
  })

  it("accepts a normal file name", () => {
    expect(validateEntryName("example.ts")).toEqual({ ok: true })
  })

  it("accepts a normal folder name", () => {
    expect(validateEntryName("components")).toEqual({ ok: true })
  })
})

describe("renameTarget", () => {
  it("replaces the final segment under the same parent", () => {
    expect(renameTarget("a/b/old.ts", "new.ts")).toBe("a/b/new.ts")
  })

  it("works at the root", () => {
    expect(renameTarget("old.ts", "new.ts")).toBe("new.ts")
  })
})
