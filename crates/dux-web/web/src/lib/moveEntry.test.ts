import { describe, expect, it, vi } from "vitest"

import { performMove } from "@/lib/moveEntry"
import type { MoveEntryDeps } from "@/lib/moveEntry"

function deps(overrides: Partial<MoveEntryDeps> = {}) {
  const calls: string[] = []
  const base: MoveEntryDeps = {
    rename: vi.fn((_from: string, _to: string) => {
      calls.push("rename")
      return Promise.resolve()
    }),
    clearTarget: vi.fn(() => calls.push("clearTarget")),
    retargetTabs: vi.fn(() => calls.push("retargetTabs")),
    revalidateDirs: vi.fn(() => calls.push("revalidateDirs")),
    refreshSearchIndex: vi.fn(() => {
      calls.push("refreshSearchIndex")
      return Promise.resolve()
    }),
    reportError: vi.fn(() => calls.push("reportError")),
    ...overrides,
  }
  return { d: base, calls }
}

describe("performMove", () => {
  it("renames to the destination directory plus the source's own name", async () => {
    const { d } = deps()
    await performMove("src/a/old.ts", "lib/util", d)
    expect(d.rename).toHaveBeenCalledWith("src/a/old.ts", "lib/util/old.ts")
  })

  it("moving to the worktree root drops the directory prefix", async () => {
    const { d } = deps()
    await performMove("src/a/old.ts", "", d)
    expect(d.rename).toHaveBeenCalledWith("src/a/old.ts", "old.ts")
  })

  // The tab bookkeeping is the half a move loses silently: without it an open
  // editor tab keeps pointing at a path that is no longer there.
  it("retargets open tabs from the old path to the new one", async () => {
    const { d } = deps()
    await performMove("src/a/old.ts", "lib", d)
    expect(d.retargetTabs).toHaveBeenCalledWith("src/a/old.ts", "lib/old.ts")
  })

  // BOTH directories: the source lost an entry and the destination gained one,
  // and the tree caches them independently, so revalidating one leaves the
  // other stale.
  it("revalidates the source and destination directories", async () => {
    const { d } = deps()
    await performMove("src/a/old.ts", "lib/util", d)
    expect(d.revalidateDirs).toHaveBeenCalledWith(["src/a", "lib/util"])
  })

  it("revalidates the worktree root as an empty directory on both sides", async () => {
    const { d } = deps()
    await performMove("old.ts", "lib", d)
    expect(d.revalidateDirs).toHaveBeenCalledWith(["", "lib"])
  })

  it("reindexes the search list and closes the dialog", async () => {
    const { d, calls } = deps()
    await performMove("src/a/old.ts", "lib", d)
    expect(calls).toEqual([
      "rename",
      "clearTarget",
      "retargetTabs",
      "revalidateDirs",
      "refreshSearchIndex",
    ])
  })

  // A refused move (an occupied destination, a containment refusal) must
  // change NOTHING on the client: the dialog stays open on its target, no tab
  // is retargeted onto a path the server did not create, and the tree is not
  // told a directory changed when it did not.
  it("touches nothing when the server refuses the move", async () => {
    const { d, calls } = deps({
      rename: vi.fn(() =>
        Promise.reject(
          new Error("refusing to rename, destination already exists: lib/a.ts"),
        ),
      ),
    })
    await performMove("src/a.ts", "lib", d)
    expect(calls).toEqual(["reportError"])
    expect(d.reportError).toHaveBeenCalledWith(
      "refusing to rename, destination already exists: lib/a.ts",
    )
    expect(d.clearTarget).not.toHaveBeenCalled()
    expect(d.retargetTabs).not.toHaveBeenCalled()
    expect(d.revalidateDirs).not.toHaveBeenCalled()
    expect(d.refreshSearchIndex).not.toHaveBeenCalled()
  })

  it("reports a non-Error rejection with a fallback message", async () => {
    const { d } = deps({ rename: vi.fn(() => Promise.reject("boom")) })
    await performMove("src/a.ts", "lib", d)
    expect(d.reportError).toHaveBeenCalledWith("could not move")
  })

  // The promise must not resolve before the reindex does: the dialog's submit
  // button keys its busy state off it.
  it("does not settle before the search reindex has", async () => {
    let release: () => void = () => {}
    const pending = new Promise<void>((resolve) => {
      release = resolve
    })
    const { d } = deps({ refreshSearchIndex: vi.fn(() => pending) })
    let settled = false
    const run = performMove("src/a.ts", "lib", d).then(() => {
      settled = true
    })
    await Promise.resolve()
    await Promise.resolve()
    expect(settled).toBe(false)
    release()
    await run
    expect(settled).toBe(true)
  })
})
