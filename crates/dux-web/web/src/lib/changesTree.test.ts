import { describe, expect, it } from "vitest"

import {
  collapseFolder,
  expandFolder,
  failFolder,
  folderKey,
  loadedChildren,
  reconcileExpansions,
  retryFolder,
  settleFolder,
  settleFolderAndReconcile,
  type Expansions,
} from "./changesTree"
import { buildChangesItems } from "./changesWindow"
import type { ChangedFileView } from "./types"

function file(path: string, status = "??"): ChangedFileView {
  return { path, status, additions: 0, deletions: 0, binary: false, diff_excluded: false }
}

function folder(path: string, count: number, extra: Partial<ChangedFileView> = {}) {
  return { ...file(path), kind: "directory", file_count: count, ...extra } as ChangedFileView
}

const EMPTY: Expansions = new Map()

describe("expanding and collapsing", () => {
  it("starts loading, settles with children, and collapse forgets the subtree", () => {
    const nm = folder("node_modules", 3)
    let exp = expandFolder(EMPTY, "unstaged", nm)
    expect(exp.get(folderKey("unstaged", "node_modules"))).toMatchObject({
      loading: true,
      children: null,
      error: null,
    })

    exp = settleFolder(exp, "unstaged", "node_modules", [
      folder("node_modules/pkg", 2),
      file("node_modules/top.js"),
    ])
    exp = expandFolder(exp, "unstaged", folder("node_modules/pkg", 2))
    exp = settleFolder(exp, "unstaged", "node_modules/pkg", [file("node_modules/pkg/a.js")])
    expect(loadedChildren(exp, "unstaged", [nm]).map((f) => f.path)).toEqual([
      "node_modules/pkg",
      "node_modules/top.js",
      "node_modules/pkg/a.js",
    ])

    exp = collapseFolder(exp, "unstaged", "node_modules")
    expect(exp.size).toBe(0)
  })

  it("ignores an answer for a folder collapsed while it was asked for", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("dist", 1))
    exp = collapseFolder(exp, "unstaged", "dist")
    expect(settleFolder(exp, "unstaged", "dist", [file("dist/a.js")])).toBe(exp)
  })

  it("shows a failure with nothing loaded, and keeps loaded children through one", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("dist", 1))
    exp = failFolder(exp, "unstaged", "dist", "the server went away")
    expect(exp.get(folderKey("unstaged", "dist"))).toMatchObject({
      loading: false,
      error: "the server went away",
    })

    exp = retryFolder(exp, "unstaged", "dist")
    expect(exp.get(folderKey("unstaged", "dist"))).toMatchObject({ loading: true, error: null })
    exp = settleFolder(exp, "unstaged", "dist", [file("dist/a.js")])
    exp = failFolder(exp, "unstaged", "dist", "a quiet refresh failed")
    expect(exp.get(folderKey("unstaged", "dist"))).toMatchObject({
      loading: false,
      error: null,
      children: [file("dist/a.js")],
    })
  })

  it("keeps the two sides apart", () => {
    let exp = expandFolder(EMPTY, "staged", folder("app", 1, { status: "A" }))
    exp = expandFolder(exp, "unstaged", folder("app", 1))
    exp = collapseFolder(exp, "staged", "app")
    expect([...exp.keys()]).toEqual([folderKey("unstaged", "app")])
  })
})

describe("reconcileExpansions", () => {
  const settled = (exp: Expansions, section: "staged" | "unstaged", path: string, rows: ChangedFileView[]) =>
    settleFolder(exp, section, path, rows)

  it("asks again, quietly, only for a folder whose row moved", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("a", 2, { fingerprint: "f1" }))
    exp = settled(exp, "unstaged", "a", [file("a/x"), file("a/y")])
    exp = expandFolder(exp, "unstaged", folder("b", 1))
    exp = settled(exp, "unstaged", "b", [file("b/z")])

    const same = reconcileExpansions(exp, [], [folder("a", 2, { fingerprint: "f1" }), folder("b", 1)])
    expect(same.refetch).toEqual([])
    expect(same.next).toBe(exp)

    const moved = reconcileExpansions(exp, [], [
      folder("a", 2, { fingerprint: "f2" }),
      folder("b", 1),
    ])
    expect(moved.refetch).toEqual([{ section: "unstaged", path: "a" }])
    // The old children stay on screen until the new ones land.
    expect(moved.next.get(folderKey("unstaged", "a"))).toMatchObject({
      loading: true,
      children: [file("a/x"), file("a/y")],
    })
    // A second pass with the same listing asks nothing more.
    expect(
      reconcileExpansions(moved.next, [], [folder("a", 2, { fingerprint: "f2" }), folder("b", 1)])
        .refetch,
    ).toEqual([])
  })

  it("forgets a folder that is no longer a folded row, and everything under it", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("a", 2))
    exp = settled(exp, "unstaged", "a", [folder("a/sub", 1)])
    exp = expandFolder(exp, "unstaged", folder("a/sub", 1))
    exp = settled(exp, "unstaged", "a/sub", [file("a/sub/q")])

    const gone = reconcileExpansions(exp, [], [file("other")])
    expect(gone.next.size).toBe(0)
    expect(gone.refetch).toEqual([])
  })

  it("follows a sub-folder through its parent's children, and asks when its count moves there", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("a", 3))
    exp = settled(exp, "unstaged", "a", [folder("a/sub", 1)])
    exp = expandFolder(exp, "unstaged", folder("a/sub", 1))
    exp = settled(exp, "unstaged", "a/sub", [file("a/sub/q")])
    // The parent refetched and its sub-folder now holds two files: the
    // parent's answer is what asks for the sub-folder again.
    const result = settleFolderAndReconcile(exp, "unstaged", "a", [folder("a/sub", 2)])
    expect(result.refetch).toEqual([{ section: "unstaged", path: "a/sub" }])
    // A listing that has not moved the parent asks nothing more.
    expect(reconcileExpansions(result.next, [], [folder("a", 3)]).refetch).toEqual([])
  })
})

describe("the flattened list", () => {
  it("puts each expanded folder's rows under it, a level deeper, with loading and failure rows", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("a", 3))
    exp = settleFolder(exp, "unstaged", "a", [folder("a/sub", 1), file("a/x")])
    exp = expandFolder(exp, "unstaged", folder("a/sub", 1))
    exp = expandFolder(exp, "unstaged", folder("b", 1))
    exp = failFolder(exp, "unstaged", "b", "no")

    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [folder("a", 3), folder("b", 1), file("c")], open: true },
      expansions: exp,
    })

    expect(
      items.map((item) =>
        item.kind === "row"
          ? `${"  ".repeat(item.depth)}${item.file.path}${item.expanded ? " (open)" : ""}`
          : item.kind === "header" || item.kind === "separator"
            ? item.kind
            : `${"  ".repeat(item.depth)}[${item.kind} ${item.dir}]`,
      ),
    ).toEqual([
      "header",
      "a (open)",
      "  a/sub (open)",
      "    [loading a/sub]",
      "  a/x",
      "b (open)",
      "  [failed b]",
      "c",
    ])
  })

  it("keys a child by its section and path, like a top-level row", () => {
    let exp = expandFolder(EMPTY, "staged", folder("a", 1, { status: "A" }))
    exp = settleFolder(exp, "staged", "a", [file("a/x", "A")])
    const items = buildChangesItems({
      staged: { files: [folder("a", 1, { status: "A" })], open: true },
      unstaged: { files: [], open: true },
      expansions: exp,
    })
    expect(items.map((item) => item.key)).toEqual(["header:staged", "staged:a", "staged:a/x"])
  })

  it("shows nothing under a closed section, expanded or not", () => {
    const exp = expandFolder(EMPTY, "unstaged", folder("a", 1))
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [folder("a", 1)], open: false },
      expansions: exp,
    })
    expect(items.map((item) => item.kind)).toEqual(["header"])
  })
})

// A folder's children are the truth for what is expanded under it: when they
// settle, the folders expanded inside it are checked against the NEW children.
describe("expanded folders inside a folder whose children settle", () => {
  function nested(): Expansions {
    let exp = expandFolder(EMPTY, "unstaged", folder("nm", 2, { fingerprint: "a" }))
    exp = settleFolder(exp, "unstaged", "nm", [folder("nm/pkg", 2, { fingerprint: "p1" })])
    exp = expandFolder(exp, "unstaged", folder("nm/pkg", 2, { fingerprint: "p1" }))
    return settleFolder(exp, "unstaged", "nm/pkg", [file("nm/pkg/a"), file("nm/pkg/b")])
  }

  it("asks again for a sub-folder that grew, once its parent's new children say so", () => {
    let exp = nested()
    const listed = reconcileExpansions(exp, [], [folder("nm", 3, { fingerprint: "b" })])
    // The parent is asked again; the sub-folder waits for the parent's answer.
    expect(listed.refetch).toEqual([{ section: "unstaged", path: "nm" }])
    exp = listed.next

    const settled = settleFolderAndReconcile(exp, "unstaged", "nm", [
      folder("nm/pkg", 3, { fingerprint: "p2" }),
    ])
    expect(settled.refetch).toEqual([{ section: "unstaged", path: "nm/pkg" }])
    expect(settled.next.get(folderKey("unstaged", "nm/pkg"))).toMatchObject({
      loading: true,
      children: [file("nm/pkg/a"), file("nm/pkg/b")],
    })
  })

  it("forgets a sub-folder that left its parent, so nothing hidden stays actionable", () => {
    const exp = nested()
    const settled = settleFolderAndReconcile(exp, "unstaged", "nm", [file("nm/top.js")])

    expect(settled.next.has(folderKey("unstaged", "nm/pkg"))).toBe(false)
    expect(settled.refetch).toEqual([])
    expect(
      loadedChildren(settled.next, "unstaged", [folder("nm", 1)]).map((f) => f.path),
    ).toEqual(["nm/top.js"])

    // Coming back later, it is a folded row again, not a stale expansion.
    const back = settleFolderAndReconcile(settled.next, "unstaged", "nm", [
      folder("nm/pkg", 2, { fingerprint: "p1" }),
    ])
    expect(back.next.has(folderKey("unstaged", "nm/pkg"))).toBe(false)
  })

  it("counts only children reachable through shown folders", () => {
    // An expansion whose parent is not expanded is unreachable, whatever it holds.
    let exp = expandFolder(EMPTY, "unstaged", folder("nm/pkg", 1))
    exp = settleFolder(exp, "unstaged", "nm/pkg", [file("nm/pkg/a")])
    expect(loadedChildren(exp, "unstaged", [folder("nm", 1)])).toEqual([])
    // And a listing reconcile forgets it.
    expect(reconcileExpansions(exp, [], [folder("nm", 1)]).next.size).toBe(0)
  })
})

describe("the flattened list's edge rows", () => {
  // A file may be called anything a path allows, colons included, so the
  // loading and failure rows' keys use a separator no path can hold.
  it("never gives a loading or failure row the key of a real row", () => {
    const exp = expandFolder(EMPTY, "unstaged", folder("node_modules", 1))
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: {
        files: [folder("node_modules", 1), file("node_modules:loading")],
        open: true,
      },
      expansions: exp,
    })
    const keys = items.map((item) => item.key)
    expect(new Set(keys).size).toBe(keys.length)
  })

  it("says so when an expanded folder turns out to hold nothing", () => {
    const exp = settleFolder(expandFolder(EMPTY, "unstaged", folder("gone", 1)), "unstaged", "gone", [])
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [folder("gone", 1)], open: true },
      expansions: exp,
    })
    expect(items.map((item) => item.kind)).toEqual(["header", "row", "empty"])
  })

  it("carries a failed quiet refresh on the folder's own row", () => {
    let exp = settleFolder(expandFolder(EMPTY, "unstaged", folder("d", 1)), "unstaged", "d", [
      file("d/a"),
    ])
    exp = failFolder(exp, "unstaged", "d", "the repository is busy")
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [folder("d", 1)], open: true },
      expansions: exp,
    })
    const row = items.find((item) => item.kind === "row" && item.file.path === "d")
    expect(row).toMatchObject({ refreshError: "the repository is busy" })
  })
})

// A folder whose first answer failed is asked again on its own when its row
// moves; while that request runs it shows it is loading, not the old failure.
describe("an automatic refetch after a failed first answer", () => {
  it("clears the failure while it asks again", () => {
    let exp = expandFolder(EMPTY, "unstaged", folder("d", 1, { fingerprint: "a" }))
    exp = failFolder(exp, "unstaged", "d", "the repository is busy")
    const moved = reconcileExpansions(exp, [], [folder("d", 2, { fingerprint: "b" })])
    expect(moved.refetch).toEqual([{ section: "unstaged", path: "d" }])
    expect(moved.next.get(folderKey("unstaged", "d"))).toMatchObject({
      loading: true,
      error: null,
    })
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [folder("d", 2, { fingerprint: "b" })], open: true },
      expansions: moved.next,
    })
    expect(items.map((item) => item.kind)).toEqual(["header", "row", "loading"])
  })
})

// A staged folder's fingerprint comes from the index, so a rename inside it
// that keeps its count still asks for its expanded contents again.
describe("an expanded staged folder", () => {
  it("is asked for again when its index fingerprint moves", () => {
    const before = folder("app", 2, { status: "A", fingerprint: "i1" })
    let exp = settleFolder(expandFolder(EMPTY, "staged", before), "staged", "app", [
      file("app/a.rs", "A"),
      file("app/b.rs", "A"),
    ])
    const after = folder("app", 2, { status: "A", fingerprint: "i2" })
    const result = reconcileExpansions(exp, [after], [])
    expect(result.refetch).toEqual([{ section: "staged", path: "app" }])
    exp = result.next
    expect(exp.get(folderKey("staged", "app"))?.loading).toBe(true)
  })
})
