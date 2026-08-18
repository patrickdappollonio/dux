import type * as React from "react"

// THE phone presentation for every anchored popup: on a small viewport a
// dropdown/⋯ menu or a popover renders as a full-width bottom sheet instead of
// an anchored popup (matching the cog's AppMenuSheet), because anchored popups
// clip against the small viewport and hide the row they came from. These
// constants are shared by ui/dropdown-menu.tsx and ui/popover.tsx so the two
// primitives cannot drift; a new popup primitive that grows a mobile branch
// should consume them rather than re-authoring the geometry.
//
// Mechanism, measured on @base-ui/react 1.5.0: a Popup cannot render outside
// its Positioner (it requires the positioner context), but a caller-supplied
// `style` on the Positioner wins over its computed floating styles
// (usePositioner merges internal styles first and useRenderElement merges the
// component's own style prop last). So the sheet keeps the Positioner and
// overrides its geometry with the fixed bottom-edge styles below — a supported
// prop, not an !important fight with inline styles.
export const SHEET_POSITIONER_STYLE: React.CSSProperties = {
  position: "fixed",
  top: "auto",
  left: 0,
  right: 0,
  bottom: 0,
  transform: "none",
}

// The sheet caps at 85dvh and scrolls internally, which leaves an uncovered
// gap at the top of the screen; the backdrop underneath covers that gap, so a
// tap there is an outside press and dismisses (base-ui's own dismissal — the
// backdrop needs no click handler). Styled after SheetOverlay in sheet.tsx.
export const SHEET_BACKDROP_CLASS =
  "fixed inset-0 z-50 bg-black/10 supports-backdrop-filter:backdrop-blur-sm transition-opacity duration-150 data-starting-style:opacity-0 data-ending-style:opacity-0 motion-reduce:transition-none"

// The popup as a bottom sheet: full width, slide-in from the bottom edge,
// internal scroll. motion-reduce drops the enter/exit animation wholesale
// (the `!` outranks the data-open/data-closed animate classes; base-ui then
// completes the open/close transition instantly). The safe-area padding keeps
// the last row above a phone's home-indicator strip.
export const SHEET_POPUP_CLASS =
  "z-50 max-h-[85dvh] w-full overflow-x-hidden overflow-y-auto overscroll-contain rounded-t-2xl bg-popover p-1 pb-[max(env(safe-area-inset-bottom),0.25rem)] text-popover-foreground shadow-lg ring-1 ring-foreground/10 outline-none duration-200 data-open:animate-in data-open:fade-in-0 data-open:slide-in-from-bottom data-closed:animate-out data-closed:fade-out-0 data-closed:slide-out-to-bottom data-closed:overflow-hidden motion-reduce:animate-none!"
