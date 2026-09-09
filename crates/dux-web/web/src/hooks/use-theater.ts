import * as React from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import {
  holdLayoutForGesture,
  type LayoutGestureHandle,
} from "@/lib/layoutGesture"
import { exitTheater, useDux } from "@/lib/store"
import {
  isTypingSurfaceElement,
  theaterEscapeAction,
  theaterTransitionMs,
} from "@/lib/theater"

/**
 * One PTY refit per toggle.
 *
 * Mounted once per shell, above every piece of chrome that animates, and it
 * watches the mode rather than the chrome: however many stacks are collapsing,
 * the gesture is one hold and one release and the terminal is re-gridded once,
 * at the geometry the gesture settled on.
 *
 * A second toggle inside the window restarts the hold rather than ending it, so
 * the handle lives in a ref and only unmounting ends it early: releasing
 * mid-animation fits the terminal at a geometry it is only passing through.
 *
 * The first run is skipped deliberately: a page that opens in theater has no
 * transition to wait out, and holding the pane's very first fit would delay the
 * terminal's first paint for nothing.
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
 * Escape leaves theater, and only where nothing else wants the keystroke.
 *
 * A document-level listener rather than a handler on the pane, so the exit works
 * from wherever focus happens to be; the one place it must not work is inside a
 * typing surface, where Escape is already the child's, which
 * `isTypingSurfaceElement` answers from the event's own target.
 *
 * Bubble phase, abstaining on an already-answered event: Base UI's dismiss hook
 * listens on the document too and calls `preventDefault` on the Escape that
 * closed a menu, a popover or a dialog, and reading that flag needs the bubble
 * phase. Waiting takes nothing from the child, which never sees a keystroke this
 * rule claims.
 */
export function useTheaterEscape(): void {
  const { theater } = useDux()

  React.useEffect(() => {
    if (!theater) return
    const onKeyDown = (ev: KeyboardEvent) => {
      const target = ev.target as { tagName?: string; isContentEditable?: boolean } | null
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
        theater: true,
      })
      if (action === "none") return
      ev.preventDefault()
      armTheaterToggleFocus()
      exitTheater()
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [theater])
}

// Where focus goes when the chrome moves: each direction destroys the control
// that was just used, leaving a keyboard user on the document body. A
// module-level flag rather than a ref threaded through both shells, because the
// header toggle and the floating pill live in different subtrees and only one of
// them exists at a time.
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
 * Only on an armed exit: a toggle that grabbed focus on every mount would pull
 * it out of the terminal on an ordinary page load.
 */
export function useTheaterToggleFocus(
  ref: React.RefObject<HTMLElement | null>,
  theater: boolean,
): void {
  useTheaterToggleFocusWhen(ref, !theater)
}

/**
 * The same hand-off for a control whose readiness is not simply "theater is
 * off". The phone's docked flap stays mounted but hidden through the return
 * flight so the choreography can measure the dock it is flying to, and focusing
 * it in that state would put the keyboard on something invisible, so the caller
 * states its own readiness.
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
 * It consumes the arm token: the press that turned the mode on destroyed the
 * control it was made on, and this is the control that replaced it. Consuming is
 * also what retires the token, which would otherwise stay armed for the whole
 * theater session and pull focus onto a later, unrelated flap.
 *
 * Unarmed, it takes focus only when nothing else holds it, so entering from the
 * input menu or from a shared link must not pull focus out of a terminal the
 * user is about to type into.
 */
export function useTheaterPillFocus(
  ref: React.RefObject<HTMLElement | null>,
): void {
  React.useEffect(() => {
    if (consumeToggleFocus()) {
      ref.current?.focus()
      return
    }
    const active = document.activeElement
    if (active !== null && active !== document.body) return
    ref.current?.focus()
  }, [ref])
}
