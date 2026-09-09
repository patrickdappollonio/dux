import type * as React from "react"

// The phone presentation for every anchored popup: on a small viewport a
// dropdown or popover renders as a full-width bottom sheet, because an anchored
// popup clips against the small viewport and hides the row it came from. Shared
// by ui/dropdown-menu.tsx and ui/popover.tsx so the two cannot drift; a new
// popup primitive with a mobile branch consumes these rather than re-authoring
// the geometry.
//
// On @base-ui/react 1.5.0 a Popup cannot render outside its Positioner, but a
// caller-supplied `style` on the Positioner wins over its computed floating
// styles, since useRenderElement merges the component's own style prop last. So
// the sheet keeps the Positioner and overrides its geometry below.
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
