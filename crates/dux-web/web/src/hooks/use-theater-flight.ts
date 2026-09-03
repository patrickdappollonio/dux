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
 * THE PHONE'S THEATER CHOREOGRAPHY, ticking.
 *
 * One phase for the whole gesture, mounted once per pane screen (the agent's
 * and the agentless terminal's alike): the flap and the pill are rendered FROM
 * it rather than each deciding for itself, so "which cluster exists right now"
 * cannot have two answers and the handoff cannot land in the gap between them.
 * A screen that hands over to another must therefore run none of its own, which
 * is why the phone's terminal spoke is a router with no hooks in it.
 *
 * A FLIGHT RUNS WHEN THE MODE MOVES, and that is the whole condition. A page
 * that OPENS in theater (a shared link, a restored pane) has no flight to run,
 * and animating one would fly a control in from a dock that was never on
 * screen. Asking whether the mode changed answers that without a first-run
 * latch, which React's development strict mode defeats by design: it invokes
 * every effect twice, and the second invocation found the latch already spent
 * and ran a phantom flight on every mount, a whole leaving gesture for anyone
 * opening a shared theater link.
 *
 * A mode that flips back mid-gesture is the flight machine's own question, not
 * the store's, so it is asked of the stage in flight (see `flightForModeFrom`).
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
