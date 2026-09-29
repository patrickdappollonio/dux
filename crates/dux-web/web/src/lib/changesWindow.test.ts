import { describe, expect, it } from "vitest"

import {
  buildChangesItems,
  changesListStructure,
  changesRowTree,
  layoutChangesItems,
  visibleChangesIndices,
  type ChangesItemHeights,
} from "./changesWindow"
import { NO_EXPANSIONS, expandFolder, settleFolder } from "./changesTree"
import type { ChangedFileView } from "./types"

function dir(path: string, count: number): ChangedFileView {
  return {
    path,
    status: "??",
    additions: 0,
    deletions: 0,
    binary: false,
    diff_excluded: false,
    kind: "directory",
    file_count: count,
  }
}

function file(path: string): ChangedFileView {
  return {
    path,
    status: "M",
    additions: 1,
    deletions: 0,
    binary: false,
  } as ChangedFileView
}

const HEIGHTS: ChangesItemHeights = { header: 30, row: 40, separator: 10 }

describe("buildChangesItems", () => {
  it("lists each non-empty section's heading and rows, with a separator between two", () => {
    const items = buildChangesItems({
      staged: { files: [file("a")], open: true },
      unstaged: { files: [file("b"), file("c")], open: true },
    })
    expect(items.map((item) => item.key)).toEqual([
      "header:staged",
      "staged:a",
      "separator",
      "header:unstaged",
      "unstaged:b",
      "unstaged:c",
    ])
  })

  it("keeps a closed section's heading and drops its rows", () => {
    const items = buildChangesItems({
      staged: { files: [file("a")], open: false },
      unstaged: { files: [file("b")], open: true },
    })
    expect(items.map((item) => item.key)).toEqual([
      "header:staged",
      "separator",
      "header:unstaged",
      "unstaged:b",
    ])
  })

  it("hides an empty section entirely and draws no separator", () => {
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [file("b")], open: true },
    })
    expect(items.map((item) => item.key)).toEqual(["header:unstaged", "unstaged:b"])
  })

  it("keys the same path apart in each section, since a file can be in both", () => {
    const items = buildChangesItems({
      staged: { files: [file("a")], open: true },
      unstaged: { files: [file("a")], open: true },
    })
    const keys = items.map((item) => item.key)
    expect(new Set(keys).size).toBe(keys.length)
  })
})

describe("changesListStructure", () => {
  it("outlines headings, each section's row range and the separator in order", () => {
    const items = buildChangesItems({
      staged: { files: [file("a"), file("b")], open: true },
      unstaged: { files: [file("c")], open: true },
    })
    expect(changesListStructure(items)).toEqual([
      { kind: "header", index: 0 },
      { kind: "rows", section: "staged", first: 1, last: 2 },
      { kind: "separator", index: 3 },
      { kind: "header", index: 4 },
      { kind: "rows", section: "unstaged", first: 5, last: 5 },
    ])
  })

  it("has no row range for a folded section", () => {
    const items = buildChangesItems({
      staged: { files: [file("a")], open: false },
      unstaged: { files: [file("c")], open: true },
    })
    expect(
      changesListStructure(items).filter((part) => part.kind === "rows"),
    ).toEqual([{ kind: "rows", section: "unstaged", first: 3, last: 3 }])
  })
})

describe("changesRowTree", () => {
  // Each expanded folder's contents sit in one container of their own, the
  // element its toggle's aria-controls names, nested the way the folders are.
  it("groups each expanded folder's contents under it, nested", () => {
    let exp = expandFolder(NO_EXPANSIONS, "unstaged", dir("a", 3))
    exp = settleFolder(exp, "unstaged", "a", [dir("a/sub", 1), file("a/x")])
    exp = expandFolder(exp, "unstaged", dir("a/sub", 1))
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [dir("a", 3), file("b")], open: true },
      expansions: exp,
    })
    // header, a, a/sub, loading, a/x, b
    expect(changesRowTree(items, 1, 5)).toEqual([
      { kind: "item", index: 1 },
      {
        kind: "folder",
        key: "unstaged:a",
        first: 2,
        last: 4,
        parts: [
          { kind: "item", index: 2 },
          {
            kind: "folder",
            key: "unstaged:a/sub",
            first: 3,
            last: 3,
            parts: [{ kind: "item", index: 3 }],
          },
          { kind: "item", index: 4 },
        ],
      },
      { kind: "item", index: 5 },
    ])
  })

  it("is a flat run of items when nothing is expanded", () => {
    const items = buildChangesItems({
      staged: { files: [], open: true },
      unstaged: { files: [file("a"), file("b")], open: true },
    })
    expect(changesRowTree(items, 1, 2)).toEqual([
      { kind: "item", index: 1 },
      { kind: "item", index: 2 },
    ])
  })
})

describe("layoutChangesItems", () => {
  it("stacks every item by its kind's height and ends with the total", () => {
    const items = buildChangesItems({
      staged: { files: [file("a")], open: true },
      unstaged: { files: [file("b")], open: true },
    })
    expect(layoutChangesItems(items, HEIGHTS)).toEqual([0, 30, 70, 80, 110, 150])
  })
})

describe("visibleChangesIndices", () => {
  const many = buildChangesItems({
    staged: { files: [], open: true },
    unstaged: {
      files: Array.from({ length: 10_000 }, (_, index) => file(`f${index}`)),
      open: true,
    },
  })
  const offsets = layoutChangesItems(many, HEIGHTS)

  it("mounts only what the viewport shows plus the overscan, however long the list", () => {
    const indices = visibleChangesIndices(offsets, 0, 400, 5)
    expect(indices[0]).toBe(0)
    // Header (30) plus rows of 40 fill 400px by index 10; five more follow.
    expect(indices.at(-1)).toBe(15)
  })

  it("follows the scroll position", () => {
    const top = offsets[5_000]!
    const indices = visibleChangesIndices(offsets, top, 400, 5)
    expect(indices[0]).toBe(4_995)
    expect(indices.at(-1)).toBe(5_015)
  })

  it("keeps the focused index mounted wherever it is, in ascending order", () => {
    const top = offsets[5_000]!
    expect(visibleChangesIndices(offsets, top, 400, 0, 3)[0]).toBe(3)
    const after = visibleChangesIndices(offsets, top, 400, 0, 9_000)
    expect(after.at(-1)).toBe(9_000)
    const sorted = [...after].sort((a, b) => a - b)
    expect(after).toEqual(sorted)
  })

  it("does not repeat the focused index when it is already in the window", () => {
    const indices = visibleChangesIndices(offsets, 0, 400, 0, 2)
    expect(indices.filter((index) => index === 2)).toHaveLength(1)
  })

  it("still mounts the last rows when scrolled past the end", () => {
    const indices = visibleChangesIndices(offsets, offsets.at(-1)! + 500, 400, 0)
    expect(indices).toEqual([many.length - 1])
  })

  it("mounts nothing for an empty list", () => {
    expect(visibleChangesIndices([0], 0, 400, 5)).toEqual([])
  })
})
