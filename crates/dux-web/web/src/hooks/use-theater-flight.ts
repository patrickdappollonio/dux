import * as React from "react"

import { useFlapHidden } from "@/hooks/use-flap-hidden"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { useDux } from "@/lib/store"
import { theaterTransitionMs, topChromeHidden } from "@/lib/theater"
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
 *
 * A full-pane cover arriving or leaving moves the chrome too, and that one is deliberately
 * an instant swap: there is nothing to fly, because the pill the cluster would travel as is
 * withheld under such a cover, and half a flight with neither end painted is worse than the
 * flap simply being there.
 *
 * A flap the user folded away is not a dock either: there is nothing on screen to fly from
 * or land on, so the mode swaps exactly as it does for a page that opens in theater.
 */
export function useTheaterFlight(coverOwnsPane: boolean): FlightPhase {
  const { theater } = useDux()
  const hidden = topChromeHidden({ theater, coverOwnsPane })
  const reducedMotion = usePrefersReducedMotion()
  const flapFolded = useFlapHidden()
  const chromeMs = React.useRef(theaterTransitionMs(reducedMotion))
  React.useEffect(() => {
    chromeMs.current = theaterTransitionMs(reducedMotion)
  }, [reducedMotion])

  const [phase, setPhase] = React.useState<FlightPhase>(() =>
    hidden ? "floating" : "docked",
  )
  const wasTheater = React.useRef(theater)
  const wasHidden = React.useRef(hidden)

  React.useEffect(() => {
    if (wasHidden.current === hidden) {
      wasTheater.current = theater
      return
    }
    // The mode moving is the user's gesture and flies; the chrome moving while
    // the mode stands still is the cover, and that one swaps.
    const ms = wasTheater.current === theater || flapFolded ? 0 : chromeMs.current
    wasTheater.current = theater
    wasHidden.current = hidden
    setPhase((live) => flightForModeFrom(live, hidden, ms))
  }, [theater, hidden, flapFolded])

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
