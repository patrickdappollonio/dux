import { describe, expect, it } from "vitest"

import {
  EDITOR_CONTENT_PANEL_ID,
  EDITOR_LAYOUT_ID,
  EXPLORER_PANEL_ID,
  explorerExpandTarget,
  isExplorerCollapsed,
  lastExpandedExplorerSize,
} from "./editorLayout"

// The collapse-state derivation for the editor's explorer panel: the panel
// library reports a Layout of {panelId: percentage}, and a collapsible panel
// collapses to its collapsedSize (the default 0%). The header toggle's icon
// and label read this, both at mount (from the persisted default layout) and
// on every onLayoutChanged.

describe("isExplorerCollapsed", () => {
  it("no stored layout means expanded (the desktop overlay starts expanded)", () => {
    expect(isExplorerCollapsed(undefined)).toBe(false)
  })

  it("a zero-size explorer entry is collapsed", () => {
    expect(
      isExplorerCollapsed({
        [EXPLORER_PANEL_ID]: 0,
        [EDITOR_CONTENT_PANEL_ID]: 100,
      }),
    ).toBe(true)
  })

  it("any real size is expanded, including the minimum", () => {
    expect(
      isExplorerCollapsed({
        [EXPLORER_PANEL_ID]: 12,
        [EDITOR_CONTENT_PANEL_ID]: 88,
      }),
    ).toBe(false)
    expect(
      isExplorerCollapsed({
        [EXPLORER_PANEL_ID]: 22,
        [EDITOR_CONTENT_PANEL_ID]: 78,
      }),
    ).toBe(false)
  })

  it("a layout missing the explorer entry is expanded, not collapsed", () => {
    // A stale or foreign stored layout must not hide the explorer.
    expect(isExplorerCollapsed({ [EDITOR_CONTENT_PANEL_ID]: 100 })).toBe(false)
  })

  it("the ids are distinct and stable (they key the persisted layout)", () => {
    expect(EXPLORER_PANEL_ID).not.toBe(EDITOR_CONTENT_PANEL_ID)
    expect(EDITOR_LAYOUT_ID.length).toBeGreaterThan(0)
  })
})

// The toggle-open width memory. `panel.expand()` falls back to minSize when
// no in-memory expand size exists (a fresh page load after collapsing), which
// would land a collapse+reload+show at a 12% sliver. So the last expanded
// size is tracked from every layout report and the toggle resizes to it.

describe("lastExpandedExplorerSize", () => {
  it("records the explorer size from an expanded layout", () => {
    expect(
      lastExpandedExplorerSize(
        { [EXPLORER_PANEL_ID]: 30, [EDITOR_CONTENT_PANEL_ID]: 70 },
        null,
      ),
    ).toBe(30)
  })

  it("a collapsed layout keeps the previous memory instead of recording 0", () => {
    expect(
      lastExpandedExplorerSize(
        { [EXPLORER_PANEL_ID]: 0, [EDITOR_CONTENT_PANEL_ID]: 100 },
        30,
      ),
    ).toBe(30)
  })

  it("a layout missing the explorer entry keeps the previous memory", () => {
    expect(
      lastExpandedExplorerSize({ [EDITOR_CONTENT_PANEL_ID]: 100 }, 27),
    ).toBe(27)
  })
})

describe("explorerExpandTarget", () => {
  it("uses the remembered width when there is one", () => {
    expect(explorerExpandTarget(30)).toBe("30%")
  })

  it("falls back to the mount default when nothing was ever recorded", () => {
    expect(explorerExpandTarget(null)).toBe("22%")
  })
})
