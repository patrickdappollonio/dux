// The editor's resizable-panel layout constants, the collapse-state derivation,
// and the explorer's own persistence.
//
// The file explorer must be the same width in both shells, the capped modal
// overlay and the uncapped standalone tab, and a percentage is two different
// widths there. So the explorer is sized in pixels and only the content pane is
// relative, and its width is persisted under dux's own key, because the panel
// library's stored `Layout` is percentages with no unit of its own.
//
// react-resizable-panels reads a bare number as pixels and a bare string as a
// percentage, so every size handed to it here is a string with its unit spelled
// out. Never a bare number: a reader cannot tell a deliberate 280 from a 22 that
// would mount the explorer as a sliver.

/// The panel group's id.
export const EDITOR_LAYOUT_ID = "dux-editor-layout"
export const EXPLORER_PANEL_ID = "editor-explorer"
export const EDITOR_CONTENT_PANEL_ID = "editor-content"

/// Where the explorer's width and collapse state live. A key of dux's own; the
/// library namespaces its own entry under `react-resizable-panels:`, so the two
/// cannot collide.
export const EXPLORER_LAYOUT_KEY = "dux-editor-explorer"

// True when the reported layout says the explorer panel is collapsed. A missing
// layout or entry reads as expanded, so a stale or foreign layout cannot hide
// the explorer. Reads the library's percentage `Layout`, which is what
// `onLayoutChanged` reports: a collapsed panel is 0 in every unit.
export function isExplorerCollapsed(
  layout: { [id: string]: number } | undefined,
): boolean {
  if (!layout) return false
  const size = layout[EXPLORER_PANEL_ID]
  // The explorer takes the default collapsedSize of 0%, and `< 1` is that zero
  // plus float slop. An explicit collapsedSize must change this threshold.
  return typeof size === "number" && size < 1
}

// The width the explorer mounts at when nothing was stored, and the fallback the
// toggle expands to. Pixels, so both shells render the same tree.
export const EXPLORER_DEFAULT_SIZE_PX = 280

// The explorer's minimum expanded width, in pixels: below roughly this a
// nested path is all ellipsis and the row actions crowd the name.
export const EXPLORER_MIN_SIZE_PX = 200

// The content pane's minimum width, a percentage of the group: the library
// requires at least one panel that keeps its relative size when the group
// resizes, and this also caps the phone expand target below.
export const EDITOR_CONTENT_MIN_SIZE = 30

// The values handed to the Panel props, each with its unit spelled out; see the
// units note in this file's header.
export const EXPLORER_DEFAULT_SIZE_PROP = `${EXPLORER_DEFAULT_SIZE_PX}px`
export const EXPLORER_MIN_SIZE_PROP = `${EXPLORER_MIN_SIZE_PX}px`
export const EDITOR_CONTENT_MIN_SIZE_PROP = `${EDITOR_CONTENT_MIN_SIZE}%`

/// What dux persists about the explorer: the width it had while expanded, in
/// pixels, and whether it is currently collapsed. The two are independent on
/// purpose, so collapsing and reopening restores the width rather than the
/// minimum.
export interface StoredExplorerLayout {
  px: number
  collapsed: boolean
}

// Read the stored explorer layout out of a raw storage value. Anything not of
// this exact shape is discarded rather than converted, and the explorer falls
// back to its default width: a percentage cannot become a pixel width without
// the width of the shell that wrote it.
export function parseExplorerLayout(
  raw: string | null | undefined,
): StoredExplorerLayout | null {
  if (!raw) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (typeof parsed !== "object" || parsed === null) return null
  const { px, collapsed } = parsed as { px?: unknown; collapsed?: unknown }
  if (typeof px !== "number" || !Number.isFinite(px) || px <= 0) return null
  if (typeof collapsed !== "boolean") return null
  return { px, collapsed }
}

/// The value written back to storage.
export function serializeExplorerLayout(state: StoredExplorerLayout): string {
  return JSON.stringify({ px: state.px, collapsed: state.collapsed })
}

// The `defaultSize` the explorer panel mounts with: the stored pixel width, or
// the default. A stored width below the minimum cannot come from a live drag,
// which the library clamps or snaps, so it is an artifact and is ignored.
export function explorerMountSize(
  stored: StoredExplorerLayout | null,
): string {
  if (stored === null || stored.px < EXPLORER_MIN_SIZE_PX) {
    return EXPLORER_DEFAULT_SIZE_PROP
  }
  return `${stored.px}px`
}

// The layout handed to the panel group at mount, for the one case of starting
// collapsed, where the explorer must mount at a true zero with no frame rendered
// expanded and no race with the library's deferred initial layout. Every other
// case returns undefined, letting the panel's pixel `defaultSize` decide.
export function editorMountLayout(
  startCollapsed: boolean,
): { [id: string]: number } | undefined {
  if (!startCollapsed) return undefined
  return { [EXPLORER_PANEL_ID]: 0, [EDITOR_CONTENT_PANEL_ID]: 100 }
}

// Fold a reported pixel width into the last-expanded-width memory. A width
// that is not a usable number, or one below the minimum, keeps the previous
// memory: the panel reports 0 while collapsed, and it reports 0 in jsdom,
// where nothing has a width at all, and neither is a width the user chose.
export function nextExpandedExplorerPx(
  reported: number | null | undefined,
  prev: number | null,
): number | null {
  if (typeof reported !== "number" || !Number.isFinite(reported)) return prev
  const px = Math.round(reported)
  if (px < EXPLORER_MIN_SIZE_PX) return prev
  return px
}

// What the toggle passes to `panel.resize()` when opening a collapsed explorer,
// as a string with its unit. Not `panel.expand()`, which falls back to minSize
// when no in-memory expand size exists, opening the explorer at its minimum.
//
// On a phone the remembered width is ignored and the target stays a percentage,
// because a fixed 280px leaves too little content pane on a narrow viewport. It
// is 100% minus the content pane's minimum, the widest the group's constraints
// permit.
export function explorerExpandTarget(
  lastExpandedPx: number | null,
  mobile = false,
): string {
  if (mobile) return `${100 - EDITOR_CONTENT_MIN_SIZE}%`
  return `${lastExpandedPx ?? EXPLORER_DEFAULT_SIZE_PX}px`
}
