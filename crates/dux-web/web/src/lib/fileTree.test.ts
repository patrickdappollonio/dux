import { describe, expect, it } from "vitest"
import { ancestorDirs, dirsToLoadFor, flattenLazy } from "./fileTree"
import type { DirEntry, DirState } from "./fileTree"

function file(path: string): DirEntry {
  const name = path.split("/").pop() ?? path
  return { name, path, is_dir: false, is_symlink: false, expandable: false }
}

function dir(path: string): DirEntry {
  const name = path.split("/").pop() ?? path
  return { name, path, is_dir: true, is_symlink: false, expandable: true }
}

describe("ancestorDirs", () => {
  it("returns each parent directory path", () => {
    expect(ancestorDirs("a/b/c.ts")).toEqual(["a", "a/b"])
  })

  it("returns nothing for a root-level file", () => {
    expect(ancestorDirs("c.ts")).toEqual([])
  })
})

describe("dirsToLoadFor", () => {
  it("returns root + each uncached ancestor top-down", () => {
    expect(dirsToLoadFor("a/b/c.ts", new Set())).toEqual(["", "a", "a/b"])
    expect(dirsToLoadFor("a/b/c.ts", new Set(["", "a"]))).toEqual(["a/b"])
    expect(dirsToLoadFor("root.txt", new Set([""]))).toEqual([])
  })
})

describe("flattenLazy", () => {
  it("returns no rows when the root is not loaded", () => {
    expect(flattenLazy(new Map(), new Set())).toEqual([])
  })

  it("lists only loaded, expanded levels and shows a loading placeholder", () => {
    const dirs = new Map<string, DirState>([
      ["", { status: "loaded", entries: [dir("src"), file("README.md")] }],
      // "src" is expanded but not yet loaded → placeholder row.
    ])
    const rows = flattenLazy(dirs, new Set(["src"]))
    expect(rows.map((r) => [r.path, r.depth, r.state])).toEqual([
      ["src", 0, "loading"],
      ["src/__loading__", 1, "loading"],
      ["README.md", 0, "idle"],
    ])
  })

  it("descends into a loaded expanded dir", () => {
    const dirs = new Map<string, DirState>([
      ["", { status: "loaded", entries: [dir("src"), file("README.md")] }],
      ["src", { status: "loaded", entries: [dir("src/app"), file("src/lib.rs")] }],
    ])
    const rows = flattenLazy(dirs, new Set(["src"]))
    expect(rows.map((r) => [r.path, r.depth, r.state])).toEqual([
      ["src", 0, "idle"],
      ["src/app", 1, "idle"],
      ["src/lib.rs", 1, "idle"],
      ["README.md", 0, "idle"],
    ])
  })

  it("does not descend into a collapsed dir even when loaded", () => {
    const dirs = new Map<string, DirState>([
      ["", { status: "loaded", entries: [dir("src")] }],
      ["src", { status: "loaded", entries: [file("src/lib.rs")] }],
    ])
    const rows = flattenLazy(dirs, new Set())
    expect(rows.map((r) => r.path)).toEqual(["src"])
    expect(rows[0].state).toBe("idle")
  })

  it("marks an errored expanded dir with an error row", () => {
    const dirs = new Map<string, DirState>([
      ["", { status: "loaded", entries: [dir("src")] }],
      ["src", { status: "error", message: "boom" }],
    ])
    const rows = flattenLazy(dirs, new Set(["src"]))
    expect(rows.map((r) => [r.path, r.depth, r.state])).toEqual([
      ["src", 0, "error"],
      ["src/__error__", 1, "error"],
    ])
  })

  it("keeps non-expandable entries as plain rows", () => {
    const escape: DirEntry = {
      name: "escape",
      path: "escape",
      is_dir: false,
      is_symlink: true,
      expandable: false,
    }
    const dirs = new Map<string, DirState>([
      ["", { status: "loaded", entries: [escape] }],
    ])
    const rows = flattenLazy(dirs, new Set())
    expect(rows).toHaveLength(1)
    expect(rows[0].isSymlink).toBe(true)
    expect(rows[0].expandable).toBe(false)
  })
})
