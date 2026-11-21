import { useCallback, useRef, type RefObject } from "react"

// What a popup's `finalFocus` may be, as base-ui takes it.
type FinalFocus<C> =
  | boolean
  | RefObject<HTMLElement | null>
  | ((closeType: C) => HTMLElement | boolean | null | void)
  | undefined

const LAYER = '[role="dialog"], [role="alertdialog"]'

/// A popup's `finalFocus`, and the ref its popup must carry, that leaves focus
/// alone while a dialog opened after the popup is up: base-ui otherwise moves
/// focus to a named target even once that dialog has taken it.
export function usePopupFinalFocus<C>(
  finalFocus: FinalFocus<C>,
): [(el: HTMLElement | null) => void, FinalFocus<C>] {
  const layers = useRef<{ popup: Element | null; before: ReadonlySet<Element> }>({
    popup: null,
    before: new Set(),
  })

  const attach = useCallback((el: HTMLElement | null) => {
    layers.current = {
      popup: el,
      before: el === null ? layers.current.before : new Set(document.querySelectorAll(LAYER)),
    }
  }, [])

  const wrapped = useCallback(
    (closeType: C) => {
      const { popup, before } = layers.current
      if (newerLayerOpen(popup, before)) return false
      if (typeof finalFocus === "function") return finalFocus(closeType)
      return finalFocus && typeof finalFocus === "object" ? finalFocus.current : finalFocus
    },
    [finalFocus],
  )

  const explicit = typeof finalFocus === "function" || (typeof finalFocus === "object" && finalFocus !== null)
  return [attach, explicit ? wrapped : finalFocus]
}

// Presence, not focus: a dialog opened in the same commit as the popup closed
// has not placed its focus yet.
function newerLayerOpen(popup: Element | null, before: ReadonlySet<Element>): boolean {
  for (const layer of document.querySelectorAll(LAYER)) {
    if (layer !== popup && !before.has(layer)) return true
  }
  return false
}
