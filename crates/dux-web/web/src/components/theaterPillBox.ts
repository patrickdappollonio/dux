import type * as React from "react"

import { flightOwnsPosition, type FlightPhase } from "@/lib/theaterFlight"
import { PILL_GRIPLESS_CLASS, type PillPosition } from "@/lib/theaterPill"
import { cn } from "@/lib/utils"

// How the pill's own box is painted and placed: the flight stage, the drag, and
// the settle between them are the only things that decide it, and they decide it
// together, so one function answers all three rather than three conditions
// spread through the element.
export function theaterPillBox(ctx: {
  flight: FlightPhase | null
  position: PillPosition | null
  dragging: boolean
  justDropped: boolean
  reducedMotion: boolean
  gripless: boolean
}): { className: string; style: React.CSSProperties | undefined } {
  const { flight, position } = ctx
  // While it flies home the flight owns the box's coordinates outright, pinning
  // the pill where it leaves and parking it on the flap's.
  const flightPlaces = flight !== null && flightOwnsPosition(flight)
  // The returning chrome's re-clamp snaps rather than settling: the top chrome
  // shrinks the surface under a resting pill, and an eased clamp crawls the pill
  // upward ahead of the gesture it belongs to.
  const positionSnaps = flightPlaces || flight === "expanding"
  return {
    className: cn(
      "absolute z-30 flex items-center gap-0.5 rounded-full border p-1",
      // Opaque, and the flap's own published fill (see `FLAP_FILL_VAR`), so
      // docked and floating are the same pixel colour and the flight has no
      // colour left to morph. A translucent surface over a terminal cannot
      // hold a band colour, so there is no blur here.
      "dux-pill-surface shadow-lg",
      // The corner it starts in until a measurement gives it real coordinates,
      // including for a pill that mounts mid-flight. The two cannot fight over
      // the same edges: parking the box pins `right` and `bottom` to `auto`.
      position === null && "right-3.5 bottom-3.5",
      // The settle after a nudge or a re-clamp, absent while a drag is live
      // (easing toward the finger lags it) and under reduced motion.
      !ctx.reducedMotion &&
        !ctx.dragging &&
        !ctx.justDropped &&
        !positionSnaps &&
        "transition-[left,top] duration-150 ease-out",
      ctx.gripless && PILL_GRIPLESS_CLASS,
      flight === "detaching" && "dux-flight-out",
      flight === "returning" && "dux-flight-in",
      flight === "attaching" && "dux-flight-attach",
    ),
    style:
      flightPlaces || position === null
        ? undefined
        : { left: position.x, top: position.y },
  }
}
