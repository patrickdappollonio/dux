import { describe, expect, it } from "vitest"
import {
  baselineSavedBuffer,
  changeSignalFor,
  emptyBuffer,
  fileLoadSeedBuffer,
  fileSignalMoved,
  isBufferStale,
  pruneByIds,
  pruneSetByIds,
  reloadedInPlace,
  shouldSkipFileLoad,
  stampsDiffer,
  unionRevalidateBatch,
} from "./editorBuffers"
import type { ChangesSliceView, TabBuffer } from "./editorBuffers"

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

// The seed constructors carry the one invariant `shouldSkipFileLoad` leans on:
// `loading` is true ONLY while a `fileApi.read` is in flight. A changed file
// opened straight into DIFF mode never runs `loadFileBuffer`; its buffer is
// seeded by the diff path from `emptyBuffer`, so if that seed reported
// `loading: true` the later switch to code mode would be skipped forever and
// the pane would spin. These lock the seeds so that regression can't return.
describe("buffer seeds and the diff-then-code switch", () => {
  it("emptyBuffer is not loading and holds no content path (no read in flight)", () => {
    const b = emptyBuffer("a.ts")
    expect(b.loading).toBe(false)
    expect(b.loadedPath).toBeNull()
    expect(b.errorPath).toBeNull()
  })

  it("fileLoadSeedBuffer is the ONLY seed that reports loading", () => {
    const b = fileLoadSeedBuffer("a.ts")
    expect(b.loading).toBe(true)
    expect(b.loadedPath).toBeNull()
  })

  it("a diff-seeded buffer does NOT block the switch to code mode", () => {
    // Reproduces the infinite-spinner bug: the tab opened in diff mode, so its
    // buffer was seeded by the diff path (emptyBuffer + a fetched diff spread on
    // top) and no file read ever ran. Switching to code mode must fire the file
    // read, i.e. shouldSkipFileLoad must be false. When emptyBuffer seeded
    // `loading: true`, this returned true and the file never loaded.
    const diffSeeded = {
      ...emptyBuffer("a.ts"),
      diff: { original: "", modified: "" } as never,
      diffLoadedPath: "a.ts",
    }
    expect(shouldSkipFileLoad(diffSeeded, "a.ts")).toBe(false)
  })
})

// --- Disk freshness ---------------------------------------------------------
//
// The bug these back: a file open in the editor never refreshed after an agent
// edited it on disk, and saving the stale buffer destroyed the agent's work.
// The event-driven half of the fix reuses the changed-files signal the diff
// view already watches, but with one crucial difference the plan review
// insisted on: a moved signal triggers a metadata CHECK, never a reload. Two
// legitimate no-op movers exist (the user's own save, and slice lifecycle
// churn), and a reload on either would throw away a draft for nothing.

function slice(over: Partial<ChangesSliceView> = {}): ChangesSliceView {
  return {
    phase: "loaded",
    staged: [],
    unstaged: [{ path: "a.ts", status: "M", additions: 3, deletions: 1 }],
    ...over,
  }
}

function loadedBuffer(over: Partial<TabBuffer> = {}): TabBuffer {
  return {
    ...emptyBuffer("a.ts"),
    loadedPath: "a.ts",
    loaded: "on disk\n",
    draft: "on disk\n",
    fileLoadedSignal: "M:3:1",
    stamp: { modified: "2026-01-01T00:00:00+00:00", size: 9 },
    ...over,
  }
}

describe("changeSignalFor", () => {
  it("is the row's status and line counts, so content edits move it", () => {
    expect(changeSignalFor(slice(), "a.ts")).toBe("M:3:1")
  })

  it("prefers the unstaged row and falls back to the staged one", () => {
    const s = slice({
      unstaged: [],
      staged: [{ path: "a.ts", status: "A", additions: 7, deletions: 0 }],
    })
    expect(changeSignalFor(s, "a.ts")).toBe("A:7:0")
  })

  it("is empty for a path git has nothing to say about, and for no path", () => {
    expect(changeSignalFor(slice(), "other.ts")).toBe("")
    expect(changeSignalFor(slice(), null)).toBe("")
    expect(changeSignalFor(null, "a.ts")).toBe("")
  })
})

// The truth table the plan review demanded, because an empty-string signal is
// ambiguous: it means "git lists nothing for this file", which is the truth
// only once the slice has actually LOADED and belongs to this session. Read off
// a loading, errored or foreign slice it means "we do not know yet", and
// treating that as absence would fire a check (and, on a clean buffer, a
// reload) every time the changes pane refetched.
describe("fileSignalMoved", () => {
  it("is false when the buffer holds the signal the slice reports", () => {
    expect(fileSignalMoved(loadedBuffer(), "a.ts", slice())).toBe(false)
  })

  it("is true when the row's counts moved (an agent edited the file)", () => {
    const s = slice({
      unstaged: [{ path: "a.ts", status: "M", additions: 9, deletions: 1 }],
    })
    expect(fileSignalMoved(loadedBuffer(), "a.ts", s)).toBe(true)
  })

  it("is true when the file left the changed list entirely (reverted)", () => {
    expect(fileSignalMoved(loadedBuffer(), "a.ts", slice({ unstaged: [] }))).toBe(
      true,
    )
  })

  it("is FALSE while the slice is still loading, even though it lists nothing", () => {
    const s = slice({ phase: "loading", unstaged: [] })
    expect(fileSignalMoved(loadedBuffer(), "a.ts", s)).toBe(false)
  })

  it("is FALSE when the slice errored", () => {
    const s = slice({ phase: "error", unstaged: [] })
    expect(fileSignalMoved(loadedBuffer(), "a.ts", s)).toBe(false)
  })

  it("is FALSE when the slice is idle", () => {
    const s = slice({ phase: "idle", unstaged: [] })
    expect(fileSignalMoved(loadedBuffer(), "a.ts", s)).toBe(false)
  })

  it("is FALSE when the slice belongs to another session (null here)", () => {
    expect(fileSignalMoved(loadedBuffer(), "a.ts", null)).toBe(false)
  })

  it("is FALSE for a buffer that has not finished loading", () => {
    expect(
      fileSignalMoved(fileLoadSeedBuffer("a.ts"), "a.ts", slice()),
    ).toBe(false)
  })

  it("is FALSE for a buffer the tab has moved off (preview-replace)", () => {
    expect(fileSignalMoved(loadedBuffer(), "b.ts", slice())).toBe(false)
    expect(fileSignalMoved(undefined, "a.ts", slice())).toBe(false)
  })

  it("is FALSE for a buffer whose load settled with an error", () => {
    const errored = loadedBuffer({ loadedPath: null, errorPath: "a.ts" })
    expect(fileSignalMoved(errored, "a.ts", slice())).toBe(false)
  })
})

// The check itself. Both halves matter: mtime alone aliases two writes inside
// one coarse clock tick, and size alone misses an edit that keeps the length.
describe("stampsDiffer", () => {
  const base = { modified: "2026-01-01T00:00:00+00:00", size: 9 }

  it("is false for the same mtime and size", () => {
    expect(stampsDiffer(base, { ...base })).toBe(false)
  })

  it("is true when only the mtime moved", () => {
    expect(stampsDiffer(base, { ...base, modified: "2026-01-02T00:00:00+00:00" })).toBe(
      true,
    )
  })

  it("is true when only the size moved (same tick, different content)", () => {
    expect(stampsDiffer(base, { ...base, size: 12 })).toBe(true)
  })

  it("treats an unknown mtime on one side as a difference", () => {
    expect(stampsDiffer(base, { modified: null, size: 9 })).toBe(true)
  })
})

// Reload IN PLACE. The buffer keeps `loadedPath`, which is what keeps
// `CodeEditor` mounted: re-seeding through the loading path would unmount it
// and DISPOSE the Monaco model, taking undo history, scroll and cursor with it.
describe("reloadedInPlace", () => {
  const fresh = {
    path: "a.ts",
    content: "the agent's work\n",
    binary: false,
    read_only: false,
    modified: "2026-02-02T00:00:00+00:00",
    size: 17,
  }

  it("keeps loadedPath so the editor is never unmounted", () => {
    const next = reloadedInPlace(loadedBuffer(), "a.ts", fresh, "M:9:1")
    expect(next.loadedPath).toBe("a.ts")
    expect(next.loading).toBe(false)
  })

  it("replaces both the baseline and the draft with the disk content", () => {
    const next = reloadedInPlace(loadedBuffer(), "a.ts", fresh, "M:9:1")
    expect(next.loaded).toBe("the agent's work\n")
    expect(next.draft).toBe("the agent's work\n")
  })

  it("re-stamps the signal and the token, so it does not re-trigger", () => {
    const next = reloadedInPlace(loadedBuffer(), "a.ts", fresh, "M:9:1")
    expect(next.fileLoadedSignal).toBe("M:9:1")
    expect(next.stamp).toEqual({ modified: fresh.modified, size: fresh.size })
    expect(next.diskState).toBe("fresh")
    expect(fileSignalMoved(next, "a.ts", slice({
      unstaged: [{ path: "a.ts", status: "M", additions: 9, deletions: 1 }],
    }))).toBe(false)
  })

  it("drops the cached diff, which now describes the previous content", () => {
    const next = reloadedInPlace(
      { ...loadedBuffer(), diffLoadedPath: "a.ts" },
      "a.ts",
      fresh,
      "M:9:1",
    )
    expect(next.diffLoadedPath).toBeNull()
  })
})

// The user's OWN save moves the changed-files signal. Re-baselining on the
// write's success body is what stops the editor mistaking its own work for
// somebody else's edit.
describe("baselineSavedBuffer", () => {
  it("adopts the saved text and the server's fresh stamp", () => {
    const next = baselineSavedBuffer(
      loadedBuffer({ draft: "typed\n" }),
      "typed\n",
      { modified: "2026-03-03T00:00:00+00:00", size: 6 },
      "M:4:1",
    )
    expect(next.loaded).toBe("typed\n")
    expect(next.stamp).toEqual({
      modified: "2026-03-03T00:00:00+00:00",
      size: 6,
    })
    expect(next.fileLoadedSignal).toBe("M:4:1")
    expect(next.diffLoadedPath).toBeNull()
    expect(next.diskState).toBe("fresh")
  })

  it("clears a banner the save has just answered", () => {
    const next = baselineSavedBuffer(
      loadedBuffer({ diskState: "changed" }),
      "typed\n",
      { modified: null, size: 6 },
      "",
    )
    expect(next.diskState).toBe("fresh")
  })
})

describe("the seeds carry the freshness fields", () => {
  it("a neutral buffer starts fresh, unstamped and unsignalled", () => {
    const b = emptyBuffer("a.ts")
    expect(b.diskState).toBe("fresh")
    expect(b.fileLoadedSignal).toBe("")
    expect(b.stamp).toEqual({ modified: null, size: null })
  })
})
