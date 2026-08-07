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
// to when no expanded width was ever recorded.
export const EXPLORER_DEFAULT_SIZE = 22

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
