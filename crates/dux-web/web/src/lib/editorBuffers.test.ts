import { describe, expect, it } from "vitest"
import { isBufferStale } from "./editorBuffers"

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
