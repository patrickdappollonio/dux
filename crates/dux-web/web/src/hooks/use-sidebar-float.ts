import type * as React from "react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"

// How long a mouse rests on a rail icon before the sidebar floats open, so a
// pointer sweeping past the rail does not flash it, and how long the pointer may
// be away from the rail and the panel before it closes.
const OPEN_DELAY_MS = 150
const CLOSE_DELAY_MS = 300

// A menu opened from inside the panel (a row's ⋯, the launcher's ⋯) renders in a
// portal outside it, so its trigger is how the panel knows one is open: Base UI
// marks an open trigger with `data-popup-open` (its `aria-expanded` is not a
// reliable signal). `aria-haspopup` keeps a tooltip trigger, which wears the
// same mark, from counting.
const OPEN_MENU = "[aria-haspopup][data-popup-open]"

// An overlay outside the panel that owns its own Escape.
const OVERLAY = '[role="dialog"], [role="alertdialog"], [role="menu"]'

function menuOpenIn(panel: HTMLElement | null): boolean {
  return panel?.querySelector(OPEN_MENU) != null
}

// Something inside the panel is in use and must not vanish from under the
// pointer leaving it: an open menu, or a text field being typed into.
function panelInUse(panel: HTMLElement | null): boolean {
  if (!panel) return false
  if (menuOpenIn(panel)) return true
  const active = document.activeElement
  return (
    active instanceof HTMLElement &&
    panel.contains(active) &&
    (active.matches("input, textarea") || active.isContentEditable)
  )
}

/**
 * The collapsed rail's floating sidebar: the full sidebar shown over the page
 * while a mouse rests on a rail icon, without changing the collapsed state.
 *
 * Only a real mouse opens it (`pointerType === "mouse"`): a tap and the mouse
 * events a browser synthesizes after one carry no such pointer, and a keyboard
 * focus is no pointer at all. `enabled` is false whenever there is no rail to
 * float from (the sidebar expanded, or the phone's sheet).
 */
export function useSidebarFloat(enabled: boolean) {
  const [open, setOpen] = useState(false)
  if (!enabled && open) setOpen(false)
  const openNow = useRef(open)
  useEffect(() => {
    openNow.current = open
  }, [open])

  // The panel element, as state: the listeners below attach to it once it exists.
  const [panel, setPanel] = useState<HTMLDivElement | null>(null)
  const openTimer = useRef<number | undefined>(undefined)
  // An opening is waiting out the hover delay.
  const pending = useRef(false)
  const closeTimer = useRef<number | undefined>(undefined)
  // Whether the mouse is over the panel (the rail is inside it).
  const inside = useRef(false)
  // After a deliberate close or cancel with the mouse still over the rail, the
  // icon under it would reopen the panel; it waits for the pointer to leave first.
  const suppressed = useRef(false)

  const cancelOpening = useCallback(() => {
    pending.current = false
    window.clearTimeout(openTimer.current)
  }, [])

  const clearTimers = useCallback(() => {
    cancelOpening()
    window.clearTimeout(closeTimer.current)
  }, [cancelOpening])

  const close = useCallback(() => {
    clearTimers()
    suppressed.current = inside.current
    setOpen(false)
  }, [clearTimers])

  const scheduleClose = useCallback(() => {
    window.clearTimeout(closeTimer.current)
    closeTimer.current = window.setTimeout(() => {
      if (inside.current || panelInUse(panel)) return
      setOpen(false)
    }, CLOSE_DELAY_MS)
  }, [panel])

  const iconHover = useMemo(
    () => ({
      onPointerEnter(event: React.PointerEvent) {
        if (!enabled || event.pointerType !== "mouse" || suppressed.current) {
          return
        }
        window.clearTimeout(openTimer.current)
        pending.current = true
        openTimer.current = window.setTimeout(() => {
          pending.current = false
          setOpen(true)
        }, OPEN_DELAY_MS)
      },
      onPointerLeave: cancelOpening,
    }),
    [enabled, cancelOpening],
  )

  // The pointer over the panel, tracked on the DOM element itself: a portaled
  // menu is outside it, which is what lets an open menu hold the panel open.
  useEffect(() => {
    if (!enabled || !panel) return
    inside.current = false
    suppressed.current = false
    const onEnter = (event: PointerEvent) => {
      if (event.pointerType !== "mouse") return
      inside.current = true
      window.clearTimeout(closeTimer.current)
    }
    const onLeave = (event: PointerEvent) => {
      if (event.pointerType !== "mouse") return
      inside.current = false
      suppressed.current = false
      cancelOpening()
      scheduleClose()
    }
    // Escape cancels an opening still waiting out the delay as well as closing an
    // open panel. Either way it is the panel's alone: taken at the window in the
    // capture phase and stopped there, so the focused terminal or compose box
    // never sees it (a stray ESC can interrupt the agent). An Escape meant for a
    // menu opened from the panel, or for a dialog, is left to that overlay.
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      if (pending.current) {
        cancelOpening()
        // The pointer is still on the icon; it has to leave and come back.
        suppressed.current = true
      } else if (openNow.current) {
        if (menuOpenIn(panel)) return
        const target = event.target
        if (
          target instanceof Element &&
          !panel.contains(target) &&
          target.closest(OVERLAY)
        ) {
          return
        }
        close()
      } else {
        return
      }
      event.preventDefault()
      event.stopPropagation()
    }
    panel.addEventListener("pointerenter", onEnter)
    panel.addEventListener("pointerleave", onLeave)
    window.addEventListener("keydown", onKeyDown, true)
    return () => {
      panel.removeEventListener("pointerenter", onEnter)
      panel.removeEventListener("pointerleave", onLeave)
      window.removeEventListener("keydown", onKeyDown, true)
      clearTimers()
    }
  }, [enabled, panel, scheduleClose, clearTimers, cancelOpening, close])

  // While open: a press outside closes it, unless a menu opened from it is open
  // (the press is in that menu, or dismisses it first), and once a menu closes or
  // a field loses focus with the pointer already gone, the leave-delay close runs.
  useEffect(() => {
    if (!open || !panel) return
    const onPointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && panel.contains(event.target)) return
      if (!menuOpenIn(panel)) close()
    }
    const onSettled = () => {
      if (!inside.current) scheduleClose()
    }
    const observer = new MutationObserver(onSettled)
    observer.observe(panel, {
      subtree: true,
      attributes: true,
      attributeFilter: ["data-popup-open"],
    })
    document.addEventListener("pointerdown", onPointerDown, true)
    panel.addEventListener("focusout", onSettled)
    return () => {
      observer.disconnect()
      document.removeEventListener("pointerdown", onPointerDown, true)
      panel.removeEventListener("focusout", onSettled)
    }
  }, [open, panel, close, scheduleClose])

  return { open, attachPanel: setPanel, iconHover, close }
}
