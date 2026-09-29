import type { LucideIcon } from "lucide-react"

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
 * `working` must already be resolved through the surface's state ladder.
 * `className` styles the icon, `frameClassName` places the frame, which is the
 * element that sits in the surrounding layout.
 */
export function WorkingGlyph({
  icon: Icon,
  working,
  className,
  frameClassName,
}: {
  icon: LucideIcon
  working: boolean
  className?: string
  frameClassName?: string
}) {
  const { bouncing, pulsing, attachBounce, attachPulse, handlers } = useWorkingCue<
    HTMLSpanElement,
    SVGSVGElement
  >(working)
  return (
    <span
      ref={attachBounce}
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
