import * as React from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { useDux } from "@/lib/store"
import { theaterTransitionMs } from "@/lib/theater"
import {
  flightForMode,
  flightHoldMs,
  flightNext,
  type FlightPhase,
} from "@/lib/theaterFlight"

/**
 * THE PHONE'S THEATER CHOREOGRAPHY, ticking.
 *
 * One phase for the whole gesture, mounted once on the agent screen: the flap
 * and the pill are rendered FROM it rather than each deciding for itself, so
 * "which cluster exists right now" cannot have two answers and the handoff
 * cannot land in the gap between them.
 *
 * The first run is skipped deliberately, exactly as `useTheaterGesture` skips
 * its own: a page that OPENS in theater (a shared link, a restored pane) has no
 * flight to run, and animating one would fly a control in from a dock that was
 * never on screen.
 *
 * The reduced-motion answer is read through a ref rather than a dependency, for
 * the same reason the layout gesture reads it that way: a system setting
 * changing mid-page must not restart a gesture over a mode that never moved.
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
  const first = React.useRef(true)

  React.useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    setPhase(flightForMode(theater, chromeMs.current))
  }, [theater])

  React.useEffect(() => {
    const hold = flightHoldMs(phase, chromeMs.current)
    if (hold === null) return
    const timer = setTimeout(
      // Guarded on the phase it was armed for: a mode flipped mid-flight
      // restarts the machine, and a timer left over from the abandoned stage
      // must not step the new one on.
      () => setPhase((live) => (live === phase ? flightNext(live) : live)),
      hold,
    )
    return () => clearTimeout(timer)
  }, [phase])

  return phase
}
