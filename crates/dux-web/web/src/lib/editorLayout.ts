// The editor's resizable-panel layout constants and the collapse-state
// derivation, kept free of React so they are unit-testable in node.
//
// Persistence is the panel library's own (`useDefaultLayout` +
// `onLayoutChanged` in EditorOverlay, storing into localStorage under
// EDITOR_LAYOUT_ID), deliberately not hand-rolled. A Layout is
// {panelId: percentage}; a collapsible panel collapses to its collapsedSize,
// which defaults to 0%.

export const EDITOR_LAYOUT_ID = "dux-editor-layout"
export const EXPLORER_PANEL_ID = "editor-explorer"
export const EDITOR_CONTENT_PANEL_ID = "editor-content"

// True when the stored/reported layout says the explorer panel is collapsed.
// Undefined (nothing stored yet) and a layout missing the explorer's entry
// both read as expanded: the desktop overlay starts expanded, and a stale or
// foreign layout must never hide the explorer by accident.
export function isExplorerCollapsed(
  layout: { [id: string]: number } | undefined,
): boolean {
  if (!layout) return false
  const size = layout[EXPLORER_PANEL_ID]
  // A collapsed collapsible panel snaps to its collapsedSize, which defaults
  // to 0% (and the explorer does not override it); `< 1` is that zero plus
  // an epsilon for float slop in the reported percentages. If the panel is
  // ever given an explicit collapsedSize, this threshold must change with it.
  return typeof size === "number" && size < 1
}

// The size the explorer mounts at, and the fallback width the toggle expands
// to when no expanded width was ever recorded. A NUMBER in the percent domain
// (0..100) because it is compared/combined with the stored Layout's values,
// which are percentages.
export const EXPLORER_DEFAULT_SIZE = 22

// The explorer's minimum expanded width and the content pane's minimum width,
// as percentages of the group (the same domain as the stored Layout).
export const EXPLORER_MIN_SIZE = 12
export const EDITOR_CONTENT_MIN_SIZE = 30

// The values actually handed to the Panel PROPS (defaultSize/minSize). STRING
// percentages on purpose: react-resizable-panels v4 reads a bare NUMBER as
// PIXELS (parseSizeAndUnit: number -> "px"; only a string is a percentage),
// so `defaultSize={22}` mounted the explorer ~22px wide, the sliver-explorer
// bug. Never pass the numeric twins to a panel prop.
export const EXPLORER_DEFAULT_SIZE_PROP = `${EXPLORER_DEFAULT_SIZE}%`
export const EXPLORER_MIN_SIZE_PROP = `${EXPLORER_MIN_SIZE}%`
export const EDITOR_CONTENT_MIN_SIZE_PROP = `${EDITOR_CONTENT_MIN_SIZE}%`

// Repair for layouts persisted while the pixel-unit bug was live: with
// `defaultSize={22}` read as 22px, `useDefaultLayout` stored the resulting
// sliver (~2% on a typical group) into localStorage, so fixing the props
// alone would keep restoring the sliver on every open. An explorer entry
// that is "expanded" (past isExplorerCollapsed's epsilon) yet below the
// minimum size cannot come from a live drag (the library clamps a drag to
// minSize or snaps it to collapsed), so it can only be that artifact, and
// the whole stored layout is dropped so the mount falls back to the string
// default sizes. Honest layouts (collapsed, at/above minimum, or missing the
// entry entirely) pass through by reference.
export function sanitizeEditorLayout(
  layout: { [id: string]: number } | undefined,
): { [id: string]: number } | undefined {
  if (!layout) return undefined
  const size = layout[EXPLORER_PANEL_ID]
  if (typeof size === "number" && size >= 1 && size < EXPLORER_MIN_SIZE) {
    return undefined
  }
  return layout
}

// Fold a layout report into the last-expanded-width memory: an expanded
// layout records the explorer's current size; a collapsed layout (or one
// missing the entry) keeps the previous memory rather than recording the
// collapsed 0%.
export function lastExpandedExplorerSize(
  layout: { [id: string]: number } | undefined,
  prev: number | null,
): number | null {
  if (!layout || isExplorerCollapsed(layout)) return prev
  const size = layout[EXPLORER_PANEL_ID]
  return typeof size === "number" ? size : prev
}

// What the toggle passes to `panel.resize()` when opening a collapsed
// explorer. A string percentage on purpose: the imperative handle reads a
// bare number as PIXELS. `panel.expand()` is not used because it falls back
// to minSize when no in-memory expand size exists (a fresh page load after
// collapsing), which would open the explorer at a 12% sliver.
export function explorerExpandTarget(lastExpanded: number | null): string {
  return `${lastExpanded ?? EXPLORER_DEFAULT_SIZE}%`
}
