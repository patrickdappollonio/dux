import { describe, expect, it } from "vitest"

import {
  EDITOR_CONTENT_MIN_SIZE,
  EDITOR_CONTENT_MIN_SIZE_PROP,
  EDITOR_CONTENT_PANEL_ID,
  EDITOR_LAYOUT_ID,
  editorMountLayout,
  explorerExpandTarget,
  EXPLORER_DEFAULT_SIZE_PROP,
  EXPLORER_DEFAULT_SIZE_PX,
  EXPLORER_LAYOUT_KEY,
  EXPLORER_MIN_SIZE_PROP,
  EXPLORER_MIN_SIZE_PX,
  EXPLORER_PANEL_ID,
  explorerMountSize,
  isExplorerCollapsed,
  nextExpandedExplorerPx,
  parseExplorerLayout,
  serializeExplorerLayout,
} from "./editorLayout"

// The collapse-state derivation for the editor's explorer panel: the panel
// library reports a Layout of {panelId: percentage}, and a collapsible panel
// collapses to its collapsedSize (the default 0%). The header toggle's icon
// and label read this on every onLayoutChanged. Collapse is the one question
// a percentage can still answer once the width itself is in pixels: a
// collapsed panel is zero in every unit.

describe("isExplorerCollapsed", () => {
  it("no reported layout means expanded (the desktop overlay starts expanded)", () => {
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

  it("any real size is expanded", () => {
    expect(
      isExplorerCollapsed({
        [EXPLORER_PANEL_ID]: 22,
        [EDITOR_CONTENT_PANEL_ID]: 78,
      }),
    ).toBe(false)
  })

  it("a layout missing the explorer entry is expanded, not collapsed", () => {
    // A stale or foreign reported layout must not hide the explorer.
    expect(isExplorerCollapsed({ [EDITOR_CONTENT_PANEL_ID]: 100 })).toBe(false)
  })

  it("the ids are distinct and stable (they key the group and the storage)", () => {
    expect(EXPLORER_PANEL_ID).not.toBe(EDITOR_CONTENT_PANEL_ID)
    expect(EDITOR_LAYOUT_ID.length).toBeGreaterThan(0)
    expect(EXPLORER_LAYOUT_KEY.length).toBeGreaterThan(0)
  })
})

// THE PROPERTY THIS FILE EXISTS FOR: the explorer is sized in PIXELS, so the
// modal overlay (capped at min(80rem, 100%-2rem)) and the standalone tab
// (uncapped) render the same tree. A percentage is two different widths there
// — 22% was ~281px in the modal and ~563px on a 2560px tab — and no
// percentage value can fix that.
describe("panel size props carry explicit units, and the explorer's are pixels", () => {
  it("the explorer's default and minimum are px, not %", () => {
    expect(EXPLORER_DEFAULT_SIZE_PROP).toBe(`${EXPLORER_DEFAULT_SIZE_PX}px`)
    expect(EXPLORER_MIN_SIZE_PROP).toBe(`${EXPLORER_MIN_SIZE_PX}px`)
    expect(EXPLORER_DEFAULT_SIZE_PROP).toMatch(/^\d+px$/)
    expect(EXPLORER_MIN_SIZE_PROP).toMatch(/^\d+px$/)
  })

  it("the content pane stays a percentage: it is the relative half of the pair", () => {
    expect(EDITOR_CONTENT_MIN_SIZE_PROP).toBe(`${EDITOR_CONTENT_MIN_SIZE}%`)
    expect(EDITOR_CONTENT_MIN_SIZE_PROP).toMatch(/^\d+(\.\d+)?%$/)
  })

  it.each([
    ["EXPLORER_DEFAULT_SIZE_PROP", EXPLORER_DEFAULT_SIZE_PROP],
    ["EXPLORER_MIN_SIZE_PROP", EXPLORER_MIN_SIZE_PROP],
    ["EDITOR_CONTENT_MIN_SIZE_PROP", EDITOR_CONTENT_MIN_SIZE_PROP],
  ])("%s is a string with its unit spelled out, never a bare number", (_n, v) => {
    // A bare number IS pixels to the library, which is what the explorer
    // wants, but a reader cannot tell a deliberate 280 from the accidental 22
    // that once mounted the explorer as a 22-pixel sliver.
    expect(typeof v).toBe("string")
    expect(v).toMatch(/(px|%)$/)
  })

  it("the default is comfortably above the minimum", () => {
    expect(EXPLORER_DEFAULT_SIZE_PX).toBeGreaterThan(EXPLORER_MIN_SIZE_PX)
  })
})

// dux persists the explorer itself rather than through the library's
// useDefaultLayout, because a library Layout is percentages by definition and
// a percentage is the thing being fixed.
describe("parseExplorerLayout", () => {
  it("round-trips through serializeExplorerLayout", () => {
    const state = { px: 341, collapsed: false }
    expect(parseExplorerLayout(serializeExplorerLayout(state))).toEqual(state)
    const collapsed = { px: 341, collapsed: true }
    expect(parseExplorerLayout(serializeExplorerLayout(collapsed))).toEqual(
      collapsed,
    )
  })

  it("nothing stored reads as nothing stored", () => {
    expect(parseExplorerLayout(null)).toBeNull()
    expect(parseExplorerLayout(undefined)).toBeNull()
    expect(parseExplorerLayout("")).toBeNull()
  })

  it("junk in storage never throws, it reads as nothing stored", () => {
    expect(parseExplorerLayout("{not json")).toBeNull()
    expect(parseExplorerLayout("[]")).toBeNull()
    expect(parseExplorerLayout("null")).toBeNull()
    expect(parseExplorerLayout("42")).toBeNull()
  })

  it("DISCARDS a layout in the old percentage shape rather than converting it", () => {
    // The migration, stated as a test. Both spellings the percentage era
    // could leave behind: the library's own namespaced entry, and a bare
    // panel-id map. Neither carries the group width that produced the
    // percentage, so there is nothing to convert it with, and preferring one
    // shell's arithmetic is the bug the pixel switch removes.
    expect(
      parseExplorerLayout('{"editor-explorer,editor-content":{"layout":[22,78]}}'),
    ).toBeNull()
    expect(parseExplorerLayout('{"editor-explorer":22,"editor-content":78}')).toBeNull()
  })

  it("rejects a half-written record rather than half-believing it", () => {
    expect(parseExplorerLayout('{"px":280}')).toBeNull()
    expect(parseExplorerLayout('{"collapsed":true}')).toBeNull()
    expect(parseExplorerLayout('{"px":"280px","collapsed":false}')).toBeNull()
    expect(parseExplorerLayout('{"px":0,"collapsed":false}')).toBeNull()
    expect(parseExplorerLayout('{"px":-10,"collapsed":false}')).toBeNull()
  })
})

describe("explorerMountSize", () => {
  it("mounts at the stored pixel width", () => {
    expect(explorerMountSize({ px: 420, collapsed: false })).toBe("420px")
  })

  it("mounts at the pixel default when nothing was stored", () => {
    expect(explorerMountSize(null)).toBe(EXPLORER_DEFAULT_SIZE_PROP)
  })

  it("ignores a stored width below the minimum (it cannot come from a drag)", () => {
    expect(explorerMountSize({ px: 30, collapsed: false })).toBe(
      EXPLORER_DEFAULT_SIZE_PROP,
    )
  })

  it("keeps the width even when the stored state is collapsed", () => {
    // Collapse is carried by the mount LAYOUT; the width is what reopening
    // restores, and it must survive being closed.
    expect(explorerMountSize({ px: 420, collapsed: true })).toBe("420px")
  })
})

describe("nextExpandedExplorerPx", () => {
  it("records a reported width, rounded", () => {
    expect(nextExpandedExplorerPx(341.4, null)).toBe(341)
  })

  it("keeps the previous memory for a width that cannot be a drag", () => {
    // 0 is what the panel reports while collapsed, and what it reports in
    // jsdom, where nothing has a width at all.
    expect(nextExpandedExplorerPx(0, 341)).toBe(341)
    expect(nextExpandedExplorerPx(EXPLORER_MIN_SIZE_PX - 1, 341)).toBe(341)
    expect(nextExpandedExplorerPx(null, 341)).toBe(341)
    expect(nextExpandedExplorerPx(undefined, 341)).toBe(341)
    expect(nextExpandedExplorerPx(NaN, 341)).toBe(341)
  })

  it("accepts a width exactly at the minimum", () => {
    expect(nextExpandedExplorerPx(EXPLORER_MIN_SIZE_PX, null)).toBe(
      EXPLORER_MIN_SIZE_PX,
    )
  })
})

describe("explorerExpandTarget", () => {
  it("reopens at the remembered pixel width", () => {
    expect(explorerExpandTarget(341)).toBe("341px")
  })

  it("falls back to the pixel default when nothing was ever recorded", () => {
    expect(explorerExpandTarget(null)).toBe(`${EXPLORER_DEFAULT_SIZE_PX}px`)
  })

  it("on a phone, expands to the widest width the layout permits, as a percentage", () => {
    // A fixed 280px on a 390px viewport leaves the content pane 110px, and
    // the reason the width is in pixels (two shells, one tree) does not apply
    // to a viewport with no room for either.
    expect(explorerExpandTarget(341, true)).toBe(
      `${100 - EDITOR_CONTENT_MIN_SIZE}%`,
    )
    expect(explorerExpandTarget(null, true)).toBe(
      `${100 - EDITOR_CONTENT_MIN_SIZE}%`,
    )
  })
})

// The layout handed to the panel group at mount exists for one case only:
// starting collapsed. Overriding the default layout (rather than only calling
// panel.collapse() from a mount effect) means there is no window where the
// panel renders expanded and no race with the library's deferred initial
// layout. Every other case returns undefined so the panel's PIXEL defaultSize
// decides the width, which a percentage layout could not express.
describe("editorMountLayout", () => {
  it("is undefined when not starting collapsed, leaving the pixel default in charge", () => {
    expect(editorMountLayout(false)).toBeUndefined()
  })

  it("mounts fully collapsed when starting collapsed", () => {
    expect(editorMountLayout(true)).toEqual({
      [EXPLORER_PANEL_ID]: 0,
      [EDITOR_CONTENT_PANEL_ID]: 100,
    })
  })

  it("a collapsed mount layout reads as collapsed for the toggle seed", () => {
    expect(isExplorerCollapsed(editorMountLayout(true))).toBe(true)
  })
})
