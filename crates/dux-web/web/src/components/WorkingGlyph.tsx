import type { LucideIcon } from "lucide-react"
import { type RefObject, useCallback } from "react"

import {
  useWorkingCue,
  WORKING_BOUNCE_ANIMATION,
  WORKING_PULSE_ANIMATION,
} from "@/hooks/use-working-cue"
import { cn } from "@/lib/utils"

/**
 * The one working glyph: the icon pulses while `working`, and a frame around
 * it bounces four times per pulse and finishes its bounce at rest on every
 * stop (a user-locked decision, CLAUDE.md "Locked by the user"). The bounce
 * rides the frame and the pulse the icon, so the pulse can yield at once to a
 * higher state without restarting or jumping the bounce; `useWorkingCue` holds
 * the reasoning.
 *
 * `working` must already be resolved through the surface's state ladder, and
 * `handover` says a higher state on that ladder is what outranks it now, so a
 * stop snaps the glyph to full opacity rather than easing it under that
 * state's own blink. `className` styles the icon, `frameClassName` places the
 * frame, which is the element that sits in the surrounding layout, and
 * `frameRef` hands the frame to `useWorkingPulseAnchor` so a state word can
 * pulse in step with it.
 */
export function WorkingGlyph({
  icon: Icon,
  working,
  handover = false,
  className,
  frameClassName,
  frameRef,
}: {
  icon: LucideIcon
  working: boolean
  handover?: boolean
  className?: string
  frameClassName?: string
  frameRef?: RefObject<HTMLSpanElement | null>
}) {
  const { bouncing, pulsing, attachBounce, attachPulse, handlers } = useWorkingCue<
    HTMLSpanElement,
    SVGSVGElement
  >(working, handover)
  const attachFrame = useCallback(
    (el: HTMLSpanElement | null) => {
      attachBounce(el)
      if (frameRef) frameRef.current = el
    },
    [attachBounce, frameRef],
  )
  return (
    <span
      ref={attachFrame}
      {...handlers}
      data-slot="working-glyph"
      className={cn(
        "inline-flex shrink-0",
        bouncing && WORKING_BOUNCE_ANIMATION,
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
