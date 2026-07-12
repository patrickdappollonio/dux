import { describe, expect, it } from "vitest"
import { isBufferStale, pruneByIds, pruneSetByIds } from "./editorBuffers"

// Component-level/logic coverage for the preview-replace stale-buffer fix:
// `EditorBody` keys its Monaco buffers by TAB ID, but `openFile` rule 2 (see
// lib/editorTabs.ts) reuses a preview tab's id while swapping its `path`. Without
// this check, a replaced preview tab would keep rendering the OLD file's buffer
// under the new path. `isBufferStale` is the single source of truth EditorBody
// consults before treating a cached buffer as usable — this test proves the logic
// a replaced tab is required to hit, standing in for a component mount test
// (Monaco cannot mount under vitest; see monacoSetup.ts).
describe("isBufferStale", () => {
  it("an absent buffer is stale (never fetched)", () => {
    expect(isBufferStale(undefined, "a.ts")).toBe(true)
  })

  it("a buffer whose path differs from the tab's CURRENT path is stale (preview-replace)", () => {
    // The tab kept its id but openFile swapped its path from a.ts to b.ts; the
    // buffer we're still holding was fetched for a.ts, so it must never render
    // as b.ts's content.
    expect(isBufferStale({ path: "a.ts" }, "b.ts")).toBe(true)
  })

  it("a buffer whose path matches the tab's current path is fresh", () => {
    expect(isBufferStale({ path: "a.ts" }, "a.ts")).toBe(false)
  })
})

// `pruneByIds` backs the finding-4 fix: EditorBody keys its `buffers` Map and
// the file/diff request-token maps by tab id, and none of them shrank on
// their own when a tab closed. This is the pure prune step those caches now
// run through in the same `[tabs]`-keyed effect that already disposes Monaco
// models by path.
describe("pruneByIds", () => {
  it("drops entries whose key is no longer live", () => {
    const map = new Map([
      ["t1", "a"],
      ["t2", "b"],
      ["t3", "c"],
    ])
    const next = pruneByIds(map, new Set(["t1", "t3"]))
    expect([...next.entries()]).toEqual([
      ["t1", "a"],
      ["t3", "c"],
    ])
  })

  it("returns the SAME map reference when nothing needs pruning", () => {
    const map = new Map([["t1", "a"]])
    const next = pruneByIds(map, new Set(["t1", "t2"]))
    expect(next).toBe(map)
  })

  it("an empty live set prunes every entry", () => {
    const map = new Map([["t1", "a"]])
    const next = pruneByIds(map, new Set())
    expect(next.size).toBe(0)
  })
})

describe("pruneSetByIds", () => {
  it("drops ids that are no longer live", () => {
    const set = new Set(["t1", "t2", "t3"])
    const next = pruneSetByIds(set, new Set(["t2"]))
    expect([...next]).toEqual(["t2"])
  })

  it("returns the SAME set reference when nothing needs pruning", () => {
    const set = new Set(["t1"])
    const next = pruneSetByIds(set, new Set(["t1", "t2"]))
    expect(next).toBe(set)
  })
})
