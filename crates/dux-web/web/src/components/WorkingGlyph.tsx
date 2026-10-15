import type { LucideIcon } from "lucide-react"
import { type RefObject, useCallback, useState } from "react"

import {
  useWorkingCue,
  WORKING_BOUNCE_ANIMATION,
  WORKING_PULSE_ANIMATION,
} from "@/hooks/use-working-cue"
import { cn } from "@/lib/utils"

// How a held look arrives once the glyph has landed: the color eases in, and
// any animation the look carries (the attention blink) starts only after it,
// so the landing reads as one movement rather than a step. The delay is
// important because the look's `animate-*` class sets the `animation`
// shorthand, which would otherwise reset it to zero.
const LANDED_LOOK_EASE =
  "transition-colors duration-200 ease-out [animation-delay:200ms]! motion-reduce:transition-none"

/**
 * The one working glyph: the icon pulses while `working`, and a frame around
 * it bounces four times per pulse and finishes its bounce at rest on every
 * stop (a user-locked decision, CLAUDE.md "Locked by the user"). The bounce
 * rides the frame and the pulse the icon, so the pulse can yield at once to a
 * higher state without restarting or jumping the bounce; `useWorkingCue` holds
 * the reasoning.
 *
 * `working` must already be resolved through the surface's state ladder.
 * `look` is the glyph's color and state animation for whatever the ladder says
 * now (the attention blink, the rail's typing tint). A new look waits while the
 * glyph is still landing from a stop, its bounce finishing and its opacity
 * easing back to full, and then eases in, so the glyph never changes color in
 * mid-air and the attention blink never multiplies the ease. Any other change
 * of look applies at once. `className` styles the icon, `frameClassName`
 * places the frame, which is the element that sits in the surrounding layout,
 * and `frameRef` hands the frame to `useWorkingPulseAnchor` so a state word
 * can pulse in step with it.
 */
export function WorkingGlyph({
  icon: Icon,
  working,
  look,
  className,
  frameClassName,
  frameRef,
}: {
  icon: LucideIcon
  working: boolean
  look?: string
  className?: string
  frameClassName?: string
  frameRef?: RefObject<HTMLSpanElement | null>
}) {
  const { bouncing, pulsing, landing, attachBounce, attachPulse, handlers } =
    useWorkingCue<HTMLSpanElement, SVGSVGElement>(working)
  const attachFrame = useCallback(
    (el: HTMLSpanElement | null) => {
      attachBounce(el)
      if (frameRef) frameRef.current = el
    },
    [attachBounce, frameRef],
  )

  const [shown, setShown] = useState(look)
  const [held, setHeld] = useState(false)
  const [eased, setEased] = useState(false)
  // A state update made during render lands on the NEXT render, so this render
  // computes the values it is about to store.
  let shownNow = shown
  let easedNow = eased
  if (look === shown) {
    if (held) setHeld(false)
  } else if (landing) {
    if (!held) setHeld(true)
  } else {
    shownNow = look
    easedNow = held
    setShown(look)
    setEased(held)
    setHeld(false)
  }

  return (
    <span
      ref={attachFrame}
      {...handlers}
      data-slot="working-glyph"
      className={cn(
        "inline-flex shrink-0",
        bouncing && WORKING_BOUNCE_ANIMATION,
        shownNow,
        easedNow && !bouncing && LANDED_LOOK_EASE,
        frameClassName,
      )}
    >
      <Icon
        ref={attachPulse}
        className={cn("shrink-0", className, pulsing && WORKING_PULSE_ANIMATION)}
      />
    </span>
  )
}
