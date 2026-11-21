import { useCallback, useRef, type RefObject } from "react"

// What a popup's `finalFocus` may be, as base-ui takes it.
type FinalFocus<C> =
  | boolean
  | RefObject<HTMLElement | null>
  | ((closeType: C) => HTMLElement | boolean | null | void)
  | undefined

const LAYER = '[role="dialog"], [role="alertdialog"]'

/// A popup's `finalFocus` that yields to a dialog opened after the popup did,
/// and the callback ref the popup must carry for it.
///
/// base-ui returns focus to an explicitly named target (a function or a ref)
/// even when focus has since moved somewhere else, and it does so only once the
/// popup's exit is over. So a menu item or a popover button that closes its
/// popup and opens a dialog had the dialog take focus first and the popup's
/// target take it back a moment later: a terminal behind the dialog then got
/// the keystrokes and Escape. Here, while a dialog layer is open that was not
/// open when the popup opened, the popup leaves focus to it. A boolean
/// `finalFocus` already behaves much this way inside base-ui.
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

// A dialog layer is open now that was not when the popup opened. Its presence
// is enough: the dialog may not have placed its own focus yet when the popup
// finishes closing in the same moment.
function newerLayerOpen(popup: Element | null, before: ReadonlySet<Element>): boolean {
  for (const layer of document.querySelectorAll(LAYER)) {
    if (layer !== popup && !before.has(layer)) return true
  }
  return false
}
