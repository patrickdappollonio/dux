import { useCallback, useLayoutEffect, useRef } from "react"

// The last control pressed or focused outside every dialog and menu. A dialog
// opened from a menu row outlives that row, so this is the control that opened
// the menu: the one focus can go back to. A press counts as well as focus
// because a tap focuses nothing in Safari.
const POPUP_LAYER = '[role="dialog"], [role="alertdialog"], [role="menu"]'
const CONTROL = 'button, a[href], input, textarea, select, [tabindex]:not([tabindex="-1"])'
let lastControl: HTMLElement | null = null

function noteControl(event: Event): void {
  if (!(event.target instanceof Element) || event.target.closest(POPUP_LAYER)) return
  const control = event.target.closest(CONTROL)
  if (control instanceof HTMLElement) lastControl = control
}

// Some store tests replace `document` with an object that takes no listeners.
if (typeof document !== "undefined" && typeof document.addEventListener === "function") {
  document.addEventListener("pointerdown", noteControl, true)
  document.addEventListener("focusin", noteControl, true)
}

export function lastPageControl(): HTMLElement | null {
  return lastControl
}

/// For a dialog or sheet root: the `onOpenChangeComplete` that, once it has
/// closed and left focus on the page body, puts focus on the page control last
/// used when it opened. base-ui hands focus back to its trigger or the element
/// focused at opening, and to nothing when that element has gone, as a menu
/// row has by the time the dialog it opened closes. Only a controlled root
/// says when it opens; an uncontrolled one has a trigger to go back to.
export function useFocusFallback(
  open: boolean | undefined,
  onOpenChangeComplete: ((open: boolean) => void) | undefined,
): (open: boolean) => void {
  const opener = useRef<HTMLElement | null>(null)

  useLayoutEffect(() => {
    if (open) opener.current = lastControl
  }, [open])

  return useCallback(
    (next: boolean) => {
      onOpenChangeComplete?.(next)
      if (next) return
      const target = opener.current
      // Two frames: base-ui reports the close before React has unmounted the
      // popup, and hands focus back only after that.
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          const active = document.activeElement
          if (active !== null && active !== document.body) return
          if (document.querySelector(POPUP_LAYER) !== null) return
          if (target?.isConnected) target.focus()
        }),
      )
    },
    [onOpenChangeComplete],
  )
}
