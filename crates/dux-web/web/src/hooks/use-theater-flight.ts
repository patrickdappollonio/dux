import * as React from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { useDux } from "@/lib/store"
import { theaterTransitionMs } from "@/lib/theater"
import {
  flightForModeFrom,
  flightHoldMs,
  flightNext,
  type FlightPhase,
} from "@/lib/theaterFlight"

/**
 * The phone's theater choreography, ticking. One phase for the whole gesture, mounted once
 * per pane screen: the flap and the pill render from it, so "which cluster exists right now"
 * cannot have two answers, and a screen that hands over to another runs none of its own.
 *
 * A flight runs when the mode MOVES, and only then: a page that opens in theater has no dock
 * to fly a control in from. That is asked as "did the mode change", never as a first-run
 * latch, which React's strict mode spends on the first of its two effect invocations and so
 * flies a phantom leaving gesture on every mount.
 *
 * A mode that flips back mid-gesture is asked of the stage in flight (`flightForModeFrom`),
 * and reduced motion is read through a ref rather than a dependency, so a system setting
 * changing mid-page cannot restart a gesture over a mode that never moved.
 */
export function useTheaterFlight(): FlightPhase {
  const { theater } = useDux()
  const reducedMotion = usePrefersReducedMotion()
  const chromeMs = React.useRef(theaterTransitionMs(reducedMotion))
  React.useEffect(() => {
    chromeMs.current = theaterTransitionMs(reducedMotion)
  }, [reducedMotion])

  const [phase, setPhase] = React.useState<FlightPhase>(() =>
    theater ? "floating" : "docked",
  )
  const wasTheater = React.useRef(theater)

  React.useEffect(() => {
    if (wasTheater.current === theater) return
    wasTheater.current = theater
    setPhase((live) => flightForModeFrom(live, theater, chromeMs.current))
  }, [theater])

  React.useEffect(() => {
    const hold = flightHoldMs(phase, chromeMs.current)
    if (hold === null) return
    const timer = setTimeout(
      // Guarded on the phase it was armed for: a timer left over from a stage the mode
      // flipped out of must not step the new one on.
      () => setPhase((live) => (live === phase ? flightNext(live) : live)),
      hold,
    )
    return () => clearTimeout(timer)
  }, [phase])

  return phase
}
