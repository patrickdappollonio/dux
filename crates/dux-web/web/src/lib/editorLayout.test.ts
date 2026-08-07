import { describe, expect, it } from "vitest"

import {
  EDITOR_CONTENT_MIN_SIZE_PROP,
  EDITOR_CONTENT_PANEL_ID,
  EDITOR_LAYOUT_ID,
  EXPLORER_DEFAULT_SIZE,
  EXPLORER_DEFAULT_SIZE_PROP,
  EXPLORER_MIN_SIZE,
  EXPLORER_MIN_SIZE_PROP,
  EXPLORER_PANEL_ID,
  explorerExpandTarget,
  isExplorerCollapsed,
  lastExpandedExplorerSize,
  sanitizeEditorLayout,
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

// The size PROPS handed to the panel components. react-resizable-panels v4
// reads a bare NUMBER as PIXELS (parseSizeAndUnit: number -> "px"); only a
// string carrying "%" is a percentage. defaultSize={22} therefore mounted the
// explorer ~22px wide, the "editor opens with a sliver explorer" bug. These
// tests pin the string-percent contract so a future bare number fails here.
describe("panel size props are string percentages, never bare numbers", () => {
  it.each([
    ["EXPLORER_DEFAULT_SIZE_PROP", EXPLORER_DEFAULT_SIZE_PROP],
    ["EXPLORER_MIN_SIZE_PROP", EXPLORER_MIN_SIZE_PROP],
    ["EDITOR_CONTENT_MIN_SIZE_PROP", EDITOR_CONTENT_MIN_SIZE_PROP],
  ])("%s is a string ending in %%", (_name, value) => {
    expect(typeof value).toBe("string")
    expect(value).toMatch(/^\d+(\.\d+)?%$/)
  })

  it("the prop constants agree with their numeric twins", () => {
    expect(EXPLORER_DEFAULT_SIZE_PROP).toBe(`${EXPLORER_DEFAULT_SIZE}%`)
    expect(EXPLORER_MIN_SIZE_PROP).toBe(`${EXPLORER_MIN_SIZE}%`)
  })

  it("the numeric twins stay in the percent domain the stored Layout uses", () => {
    // The persisted Layout maps panel id -> percentage 0..100, so every
    // numeric constant compared against it must live in that domain too.
    expect(EXPLORER_DEFAULT_SIZE).toBeGreaterThan(EXPLORER_MIN_SIZE)
    expect(EXPLORER_DEFAULT_SIZE).toBeLessThanOrEqual(100)
    expect(EXPLORER_MIN_SIZE).toBeGreaterThan(1) // above the collapsed epsilon
  })
})

// Repairing layouts persisted by the pixel-unit bug: while defaultSize was a
// bare 22 (pixels), useDefaultLayout stored the resulting sliver (~2%) into
// localStorage, so fixing the props alone would still restore the sliver on
// every open. A stored layout whose explorer is "expanded" (past the
// collapsed epsilon) yet below the minimum size can only be that artifact
// (live drags are clamped to minSize or snapped to collapsed), so it is
// dropped wholesale and the mount falls back to the default sizes.
describe("sanitizeEditorLayout", () => {
  it("keeps a healthy stored layout, same reference", () => {
    const layout = { [EXPLORER_PANEL_ID]: 22, [EDITOR_CONTENT_PANEL_ID]: 78 }
    expect(sanitizeEditorLayout(layout)).toBe(layout)
  })

  it("keeps a deliberately collapsed layout", () => {
    const layout = { [EXPLORER_PANEL_ID]: 0, [EDITOR_CONTENT_PANEL_ID]: 100 }
    expect(sanitizeEditorLayout(layout)).toBe(layout)
  })

  it("keeps a layout parked exactly at the minimum", () => {
    const layout = {
      [EXPLORER_PANEL_ID]: EXPLORER_MIN_SIZE,
      [EDITOR_CONTENT_PANEL_ID]: 100 - EXPLORER_MIN_SIZE,
    }
    expect(sanitizeEditorLayout(layout)).toBe(layout)
  })

  it("drops a sliver layout left behind by the pixel-unit bug", () => {
    // 22px on a ~1200px group persisted as ~1.9%.
    const layout = {
      [EXPLORER_PANEL_ID]: 1.9,
      [EDITOR_CONTENT_PANEL_ID]: 98.1,
    }
    expect(sanitizeEditorLayout(layout)).toBeUndefined()
  })

  it("passes undefined through", () => {
    expect(sanitizeEditorLayout(undefined)).toBeUndefined()
  })

  it("keeps a layout missing the explorer entry (foreign/stale, not a sliver)", () => {
    const layout = { [EDITOR_CONTENT_PANEL_ID]: 100 }
    expect(sanitizeEditorLayout(layout)).toBe(layout)
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
