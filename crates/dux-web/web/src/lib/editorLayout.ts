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
  // Strictly below the 12% minSize only happens when collapsed (the library
  // snaps a collapsible panel to its 0% collapsedSize); compare against a
  // small epsilon rather than exact zero to survive float noise.
  return typeof size === "number" && size < 1
}
