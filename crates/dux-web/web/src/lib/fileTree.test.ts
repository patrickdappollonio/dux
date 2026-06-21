import { describe, expect, it } from "vitest"
import { ancestorDirs, buildFileTree, FILETREE_NODE_CAP } from "./fileTree"

describe("buildFileTree", () => {
  it("nests files under their directories", () => {
    const { nodes: tree } = buildFileTree(["src/app/main.rs", "src/lib.rs", "README.md"])
    // Top level: dir `src` first, then file `README.md`.
    expect(tree.map((n) => `${n.name}:${n.isDir}`)).toEqual([
      "src:true",
      "README.md:false",
    ])
    const src = tree[0]
    // Inside src: dir `app` first, then file `lib.rs`.
    expect(src.children.map((n) => n.name)).toEqual(["app", "lib.rs"])
    expect(src.children[0].children.map((n) => n.path)).toEqual([
      "src/app/main.rs",
    ])
  })

  it("sorts directories before files, case-insensitively", () => {
    const { nodes: tree } = buildFileTree(["Zoo.txt", "apple.txt", "dir/x", "Banana.txt"])
    expect(tree.map((n) => n.name)).toEqual([
      "dir",
      "apple.txt",
      "Banana.txt",
      "Zoo.txt",
    ])
  })

  it("handles a single root-level file", () => {
    const { nodes: tree } = buildFileTree(["a.txt"])
    expect(tree).toEqual([
      { name: "a.txt", path: "a.txt", isDir: false, children: [] },
    ])
  })
})

describe("buildFileTree node cap", () => {
  it("returns capped=false for a small input", () => {
    const result = buildFileTree(["a.ts", "b.ts", "src/c.ts"])
    expect(result.capped).toBe(false)
    expect(result.nodes.length).toBeGreaterThan(0)
  })

  it("returns capped=true and truncates nodes when input exceeds cap", () => {
    // Generate more paths than the cap.
    const paths = Array.from({ length: FILETREE_NODE_CAP + 10 }, (_, i) => `f${i}.txt`)
    const result = buildFileTree(paths)
    expect(result.capped).toBe(true)
    // Node count must not exceed the cap (each flat file is one node).
    const count = result.nodes.reduce(function countNodes(acc: number, n: import("./fileTree").FileTreeNode): number {
      return acc + 1 + (n.isDir ? n.children.reduce(countNodes, 0) : 0)
    }, 0)
    expect(count).toBeLessThanOrEqual(FILETREE_NODE_CAP)
  })
})

describe("ancestorDirs", () => {
  it("returns each parent directory path", () => {
    expect(ancestorDirs("a/b/c.ts")).toEqual(["a", "a/b"])
  })

  it("returns nothing for a root-level file", () => {
    expect(ancestorDirs("c.ts")).toEqual([])
  })
})
