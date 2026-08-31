import { describe, expect, it } from "vitest"

import { changesSummary } from "./changesSummary"
import type { ChangesSlice } from "./store"

function slice(over: Partial<ChangesSlice> = {}): ChangesSlice {
  return {
    sessionId: "s1",
    phase: "loaded",
    rev: 1,
    staged: [],
    unstaged: [],
    error: null,
    ...over,
  }
}

function file(path: string, status: string): ChangesSlice["staged"][number] {
  return { path, status, additions: 0, deletions: 0, binary: false }
}

describe("changesSummary", () => {
  it("has no summary at all when no agent is in view", () => {
    // The phone only draws the ±N control on an agent screen; the desktop
    // header must agree, so a focused project or standalone terminal gets the
    // bare icon rather than a count belonging to nothing.
    expect(changesSummary(slice(), null)).toBeNull()
    expect(changesSummary(slice(), undefined)).toBeNull()
  })

  it("counts staged and unstaged together", () => {
    const s = slice({
      staged: [file("a.rs", "M"), file("b.rs", "A")],
      unstaged: [file("c.rs", "??")],
    })
    const summary = changesSummary(s, "s1")
    expect(summary).toEqual({
      count: 3,
      label: "±3",
      countLabel: "3 changed files",
    })
  })

  it("counts a deletion and a rename like any other changed file", () => {
    // The summary is a FILE COUNT, not a line-delta and not a per-status
    // breakdown, so no status code is weighted or dropped.
    const s = slice({
      staged: [file("gone.rs", "D"), file("new.rs", "R")],
      unstaged: [],
    })
    expect(changesSummary(s, "s1")?.count).toBe(2)
  })

  it("reads a clean worktree as the zero state, the way the phone does", () => {
    expect(changesSummary(slice(), "s1")).toEqual({
      count: 0,
      label: "±0",
      countLabel: "0 changed files",
    })
  })

  it("reads zero rather than another session's numbers", () => {
    // The store slice only ever holds the SELECTED session's files, so a slice
    // pointing elsewhere is stale for this agent.
    const s = slice({ sessionId: "other", staged: [file("a.rs", "M")] })
    expect(changesSummary(s, "s1")?.count).toBe(0)
  })

  it("reads zero while the fetch is still in flight or failed", () => {
    const loading = slice({ phase: "loading", staged: [file("a.rs", "M")] })
    expect(changesSummary(loading, "s1")?.count).toBe(0)
    const failed = slice({ phase: "error", staged: [file("a.rs", "M")] })
    expect(changesSummary(failed, "s1")?.count).toBe(0)
  })

  it("survives a store that has not booted its slice yet", () => {
    expect(changesSummary(null, "s1")?.count).toBe(0)
    expect(changesSummary(undefined, "s1")?.count).toBe(0)
  })
})
