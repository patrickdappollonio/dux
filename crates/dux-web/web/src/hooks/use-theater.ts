import * as React from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import {
  holdLayoutForGesture,
  type LayoutGestureHandle,
} from "@/lib/layoutGesture"
import { exitTheater, useDux } from "@/lib/store"
import {
  isTypingSurfaceElement,
  peekTheaterTabs,
  theaterEscapeAction,
  theaterTransitionMs,
} from "@/lib/theater"

/**
 * ONE PTY REFIT PER TOGGLE.
 *
 * Mounted once per shell, above every piece of chrome that animates. It watches
 * the mode rather than the chrome, so however many stacks are collapsing (the
 * desktop shell has two, the header and the pane's own band-plus-strip) the
 * gesture is one hold and one release, and the terminal is re-gridded once at
 * the geometry the gesture settled on.
 *
 * A SECOND TOGGLE INSIDE THE WINDOW RE-ARMS IT rather than ending it. Written
 * as an ordinary effect cleanup, the first gesture's canceller ran the moment
 * the mode changed again, which released the hold in the middle of an animation
 * and fitted the terminal at a geometry it was only passing through. So the
 * handle lives in a ref, a mode change restarts it, and only unmounting ends it
 * early.
 *
 * The first run is skipped deliberately: a page that OPENS in theater (a shared
 * link, a restored pane) has no transition to wait out, and holding the pane's
 * very first fit would delay the terminal's first paint for nothing.
 */
export function useTheaterGesture(): void {
  const { theater } = useDux()
  const reducedMotion = usePrefersReducedMotion()
  // Read through a ref so a system setting changing mid-page does not re-fire
  // the effect and start a second gesture over a mode that never moved.
  const reducedRef = React.useRef(reducedMotion)
  React.useEffect(() => {
    reducedRef.current = reducedMotion
  }, [reducedMotion])
  const first = React.useRef(true)
  const gesture = React.useRef<LayoutGestureHandle | null>(null)

  React.useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    const ms = theaterTransitionMs(reducedRef.current)
    const running = gesture.current
    if (running) {
      running.restart(ms)
      return
    }
    gesture.current = holdLayoutForGesture(ms, () => {
      gesture.current = null
    })
  }, [theater])

  // The only early end. A page teardown mid-transition must not leave a hold
  // behind: every pane would stop fitting for the rest of the page's life.
  React.useEffect(
    () => () => {
      gesture.current?.cancel()
      gesture.current = null
    },
    [],
  )
}

/**
 * ESCAPE LEAVES THEATER, and only where nothing else wants the keystroke.
 *
 * A document-level listener rather than a handler on the pane: the exit has to
 * work from wherever focus happens to be (the pill, a header control, the page
 * body), and the one place it must NOT work is inside a typing surface, where
 * Escape is already the child's. `isTypingSurfaceElement` answers that from the
 * event's own target, which covers both the compose textarea and xterm's helper
 * textarea without either of them having to know theater exists.
 *
 * BUBBLE PHASE, and it abstains on an already-answered event. Base UI's dismiss
 * hook listens on the document too and calls `preventDefault` on the Escape that
 * closed a menu, a popover or a dialog; in capture phase this rule ran first and
 * one press both closed the overlay and left the mode. Reading that flag needs
 * the bubble phase, and waiting takes nothing from the child, which never sees a
 * keystroke this rule claims.
 *
 * The pill's folded-out tab strip is the other thing that wants Escape, and it
 * is the innermost: it collapses first, and the next press leaves theater.
 */
export function useTheaterEscape(): void {
  const { theater } = useDux()

  React.useEffect(() => {
    if (!theater) return
    const onKeyDown = (ev: KeyboardEvent) => {
      const target = ev.target as { tagName?: string; isContentEditable?: boolean } | null
      const tabs = peekTheaterTabs()
      const action = theaterEscapeAction({
        type: ev.type,
        key: ev.key,
        ctrlKey: ev.ctrlKey,
        shiftKey: ev.shiftKey,
        altKey: ev.altKey,
        metaKey: ev.metaKey,
        isComposing: ev.isComposing,
        keyCode: ev.keyCode,
        inTypingSurface: isTypingSurfaceElement(target),
        defaultPrevented: ev.defaultPrevented,
        tabsExpanded: tabs?.expanded() === true,
        theater: true,
      })
      if (action === "none") return
      ev.preventDefault()
      if (action === "collapse-tabs") {
        tabs?.collapse()
        return
      }
      armTheaterToggleFocus()
      exitTheater()
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [theater])
}

// WHERE FOCUS GOES WHEN THE CHROME MOVES. Each direction destroys the control
// that was just used, so with nothing done about it a keyboard user is left on
// the document body, the far end of the page from what they were doing.
//
// A module-level flag rather than a ref threaded through both shells, for the
// same reason the tab registry is one: the header toggle and the floating pill
// live in different subtrees, and only one of them exists at a time.
let toggleFocusArmed = false

/** Ask the header toggle to take focus as soon as it comes back. */
export function armTheaterToggleFocus(): void {
  toggleFocusArmed = true
}

function consumeToggleFocus(): boolean {
  const armed = toggleFocusArmed
  toggleFocusArmed = false
  return armed
}

/**
 * The header toggle taking focus back after an exit that was not its own press.
 *
 * Only on an ARMED exit: a toggle that grabbed focus on every mount would pull
 * it out of the terminal on an ordinary page load.
 */
export function useTheaterToggleFocus(
  ref: React.RefObject<HTMLElement | null>,
  theater: boolean,
): void {
  useTheaterToggleFocusWhen(ref, !theater)
}

/**
 * The same hand-off, for a control whose "I am on screen and settled" is not
 * simply "theater is off".
 *
 * The phone's docked flap is mounted (hidden) through the whole return flight
 * so the choreography can measure the dock it is flying to; focusing it in that
 * state would put the keyboard on something invisible. Its readiness is the
 * flight's, not the mode's, so it says so itself.
 */
export function useTheaterToggleFocusWhen(
  ref: React.RefObject<HTMLElement | null>,
  ready: boolean,
): void {
  React.useEffect(() => {
    if (!ready) return
    if (!consumeToggleFocus()) return
    ref.current?.focus()
  }, [ref, ready])
}

/**
 * The pill's exit button taking focus when the chrome leaves.
 *
 * ONLY when nothing else holds it. Entering from the header button destroys
 * that button, so focus falls to the body and the pill is the nearest thing to
 * what the user was doing; entering from the input menu, or from a shared link
 * that opens straight into theater, leaves focus somewhere real, and the pill
 * must not pull it out of a terminal the user is about to type into.
 */
export function useTheaterPillFocus(
  ref: React.RefObject<HTMLElement | null>,
): void {
  React.useEffect(() => {
    const active = document.activeElement
    if (active !== null && active !== document.body) return
    ref.current?.focus()
  }, [ref])
}
