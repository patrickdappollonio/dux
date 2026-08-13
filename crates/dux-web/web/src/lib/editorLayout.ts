// The editor's resizable-panel layout constants, the collapse-state
// derivation, and the explorer's own persistence, kept free of React so they
// are unit-testable in node.
//
// THE POINT OF THIS MODULE, in one sentence: the file explorer is the SAME
// WIDTH in both shells. The editor body composes into two of them, the modal
// overlay (capped at `min(80rem, 100% - 2rem)`) and the standalone whole-tab
// surface (uncapped), and a PERCENTAGE means two different widths there: 22%
// of a capped modal is ~281px while 22% of a 2560px tab is ~563px, the same
// tree rendered at double the width depending on which door you came in by.
// So the explorer is sized in PIXELS and only the content pane is relative.
//
// UNITS, because the panel library takes both and the reader has to know
// which is which. react-resizable-panels v4 reads a bare NUMBER as PIXELS
// (parseSizeAndUnit: number -> "px") and a string as whatever unit it carries
// (a bare string is a percentage). Every size dux hands it is an EXPLICIT
// string with its unit spelled out: `"280px"` for the explorer, `"30%"` for
// the content pane. Never a bare number: it would mean pixels, which is what
// the explorer wants, but a reader cannot tell a deliberate 280 from the
// accidental 22 that once mounted the explorer as a 22-pixel sliver.
//
// PERSISTENCE is dux's own here, deliberately, and this is a departure from
// "reuse before invent". The library's `useDefaultLayout` hook persists a
// `Layout`, and a `Layout` is defined as a map of panel id to PERCENTAGE
// (0..100) with no unit of its own, so a width stored through it comes back
// as a fraction of whichever shell wrote it and the two shells disagree again
// the moment either one is resized. There is no percent value that fixes
// that. So the explorer's width is stored, in pixels, under dux's own key.

/// The panel group's id. Also the stem of the abandoned library storage key
/// (see EXPLORER_LAYOUT_KEY).
export const EDITOR_LAYOUT_ID = "dux-editor-layout"
export const EXPLORER_PANEL_ID = "editor-explorer"
export const EDITOR_CONTENT_PANEL_ID = "editor-content"

/// Where the explorer's width and collapse state live.
///
/// A key of dux's own rather than the library's. `useDefaultLayout` namespaces
/// its entry as `react-resizable-panels:dux-editor-layout:<panel ids>`, so the
/// two can never collide; a value written by the previous, percentage-based
/// version of this module is simply left behind, unread, and
/// `parseExplorerLayout` explains what happens to a value in the OLD shape
/// that does reach it.
export const EXPLORER_LAYOUT_KEY = "dux-editor-explorer"

// True when the stored/reported layout says the explorer panel is collapsed.
// Undefined (nothing stored yet) and a layout missing the explorer's entry
// both read as expanded: the desktop overlay starts expanded, and a stale or
// foreign layout must never hide the explorer by accident.
//
// This still reads the library's percentage `Layout`, because that is what
// `onLayoutChanged` reports and collapse is the one question a percentage can
// still answer: a collapsed panel is 0 in every unit.
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

// The width the explorer mounts at when nothing was ever stored, and the
// fallback the toggle expands to. PIXELS, so the modal and the standalone tab
// render the same tree: 280px is the measured width of the previous 22% in
// the capped modal (~281px), which is the shell the explorer was tuned in.
export const EXPLORER_DEFAULT_SIZE_PX = 280

// The explorer's minimum expanded width, in pixels: below roughly this a
// nested path is all ellipsis and the row actions crowd the name.
export const EXPLORER_MIN_SIZE_PX = 200

// The content pane's minimum width, still a PERCENTAGE of the group. It is
// the relative half of the pair (the library requires at least one panel that
// preserves its relative size when the group resizes) and it is also what
// caps the phone expand target below.
export const EDITOR_CONTENT_MIN_SIZE = 30

// The values actually handed to the Panel PROPS (defaultSize/minSize).
// Explicit units on every one; see the units note in this file's header for
// why never a bare number.
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

// Read the stored explorer layout out of a raw storage value.
//
// MIGRATION. Anything that is not this exact shape is DISCARDED and the
// explorer falls back to its default width. That includes every layout
// persisted by the previous percentage-based version of this module (both the
// library's `{"editor-explorer,editor-content": {layout: [22, 78]}}` under its
// own namespaced key and a bare `{"editor-explorer": 22}`). Discarded rather
// than converted, because a percentage cannot be turned into a pixel width
// without the width of the group that produced it, that group belonged to
// whichever shell happened to write it last, and preferring one shell's
// arithmetic is exactly the bug the pixel switch exists to remove. The cost of
// discarding is one explorer that opens at 280px and is dragged once.
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
// the pixel default when nothing honest was stored. A stored width BELOW the
// minimum cannot come from a live drag (the library clamps a drag to minSize
// or snaps it to collapsed), so it is an artifact and the default is used
// instead.
export function explorerMountSize(
  stored: StoredExplorerLayout | null,
): string {
  if (stored === null || stored.px < EXPLORER_MIN_SIZE_PX) {
    return EXPLORER_DEFAULT_SIZE_PROP
  }
  return `${stored.px}px`
}

// The layout handed to the panel group at mount. It exists for ONE case:
// starting COLLAPSED, where the explorer must mount at a true zero. A
// defaultLayout override rather than only an imperative `panel.collapse()`
// from a mount effect: mounting collapsed leaves no frame where the panel
// renders expanded and cannot race the library's own deferred initial layout.
// Every other case returns undefined, letting the panel's pixel `defaultSize`
// decide the width; a percentage layout could not express it.
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

// What the toggle passes to `panel.resize()` when opening a collapsed
// explorer. A string with its unit on purpose (a bare number would be pixels
// by accident rather than by decision). `panel.expand()` is not used because
// it falls back to minSize when no in-memory expand size exists (a fresh page
// load after collapsing), which would open the explorer at the minimum.
//
// On a phone the remembered/default width is ignored entirely and the target
// stays a PERCENTAGE: a fixed 280px on a 390px viewport leaves the content
// pane 110px, and the point of the pixel width (two shells, one width) does
// not apply to a viewport that has no room for it. The phone target is 100%
// minus the content pane's minimum, the widest width the group's constraints
// permit (70% ≈ 273px at 390px), chosen because anything larger is clamped
// back by EDITOR_CONTENT_MIN_SIZE anyway.
export function explorerExpandTarget(
  lastExpandedPx: number | null,
  mobile = false,
): string {
  if (mobile) return `${100 - EDITOR_CONTENT_MIN_SIZE}%`
  return `${lastExpandedPx ?? EXPLORER_DEFAULT_SIZE_PX}px`
}
