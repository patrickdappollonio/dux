import { describe, expect, it } from "vitest"
import {
  isBufferStale,
  pruneByIds,
  pruneSetByIds,
  shouldSkipFileLoad,
  unionRevalidateBatch,
} from "./editorBuffers"

// Component-level/logic coverage for the preview-replace stale-buffer fix:
// `EditorBody` keys its Monaco buffers by TAB ID, but `openFile` rule 2 (see
// lib/editorTabs.ts) reuses a preview tab's id while swapping its `path`. Without
// this check, a replaced preview tab would keep rendering the OLD file's buffer
// under the new path. `isBufferStale` is the single source of truth EditorBody
// consults before treating a cached buffer as usable, this test proves the logic
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

// `unionRevalidateBatch` backs the finding-4 fix: `EditorBody`'s
// `revalidateDirs` used to do a plain `setTreeRevalidate({ dirs, nonce })`,
// which silently dropped a same-tick prior batch's dirs when two mutations
// (e.g. a rename touching two parent dirs, or a rapid create + rename) each
// called `revalidateDirs` before React flushed a render in between: React
// batches the two `setState` calls, so only the LAST plain assignment survives
// and the first batch's dirs are lost, meaning `FileTree` never re-fetches
// them. A functional update whose updater unions the new dirs into whatever
// batch is already pending fixes this because updaters run in call order even
// within one batch.
describe("unionRevalidateBatch", () => {
  it("unions a new batch's dirs into a same-tick prior batch, deduped", () => {
    const prev = { dirs: ["a", "b"], nonce: 1 }
    const next = unionRevalidateBatch(prev, ["b", "c"], 2)
    expect(next.nonce).toBe(2)
    expect([...next.dirs].sort()).toEqual(["a", "b", "c"])
  })

  it("starts fresh when there is no pending batch", () => {
    const next = unionRevalidateBatch(null, ["a"], 1)
    expect(next).toEqual({ dirs: ["a"], nonce: 1 })
  })

  it("stamps the latest nonce even though dirs accumulate", () => {
    const b1 = unionRevalidateBatch(null, ["a"], 1)
    const b2 = unionRevalidateBatch(b1, ["b"], 2)
    const b3 = unionRevalidateBatch(b2, ["a"], 3)
    expect(b3.nonce).toBe(3)
    expect([...b3.dirs].sort()).toEqual(["a", "b"])
  })
})

// `shouldSkipFileLoad` backs the finding-2 fix: a failed `fileApi.read` left
// `loadedPath: null` and `loading: false`, so EditorBody's load effect saw
// "not loaded, not loading" on every render while the tab stayed active and
// fired a fresh `fileApi.read` forever (reachable via a delete/rename race,
// or any plain 404). A settled error must count as "don't auto-retry": the
// error pane is the surface, and only an explicit Retry click (or opening a
// different path) should trigger another fetch. `errorPath` records which
// path a load last failed FOR, mirroring how `loadedPath` records which path
// last succeeded; both are compared against the tab's CURRENT path so a
// preview-replace onto the same failing path still gets skipped only for
// that exact path.
describe("shouldSkipFileLoad", () => {
  function buffer(over: Partial<Parameters<typeof shouldSkipFileLoad>[0]> = {}) {
    return {
      path: "a.ts",
      loadedPath: null,
      loading: false,
      errorPath: null,
      ...over,
    }
  }

  it("does not skip when there is no buffer yet", () => {
    expect(shouldSkipFileLoad(undefined, "a.ts")).toBe(false)
  })

  it("does not skip a stale buffer (preview-replace swapped the path)", () => {
    expect(shouldSkipFileLoad(buffer({ path: "old.ts" }), "a.ts")).toBe(false)
  })

  it("skips while a fetch for this exact path is already in flight", () => {
    expect(shouldSkipFileLoad(buffer({ loading: true }), "a.ts")).toBe(true)
  })

  it("skips once this exact path has successfully loaded", () => {
    expect(shouldSkipFileLoad(buffer({ loadedPath: "a.ts" }), "a.ts")).toBe(
      true,
    )
  })

  it("skips a settled error for this exact path, breaking the retry loop", () => {
    expect(shouldSkipFileLoad(buffer({ errorPath: "a.ts" }), "a.ts")).toBe(
      true,
    )
  })

  it("does not skip a settled error for a DIFFERENT path (new open, retry)", () => {
    expect(
      shouldSkipFileLoad(
        buffer({ path: "b.ts", errorPath: "b.ts" }),
        "b.ts",
      ),
    ).toBe(true)
    // Same buffer, but the tab moved on to a different path: must refetch.
    expect(
      shouldSkipFileLoad(buffer({ path: "b.ts", errorPath: "b.ts" }), "c.ts"),
    ).toBe(false)
  })
})
