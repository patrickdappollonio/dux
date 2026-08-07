import { describe, expect, it } from "vitest"

import {
  EDITOR_CONTENT_PANEL_ID,
  EDITOR_LAYOUT_ID,
  EXPLORER_PANEL_ID,
  isExplorerCollapsed,
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
