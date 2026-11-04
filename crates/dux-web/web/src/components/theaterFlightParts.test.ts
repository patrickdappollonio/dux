import { describe, expect, it } from "vitest"

import { FLIGHT_TRAVEL_HOLD_MS } from "@/lib/theaterFlight"

import { FILLET_FADE_MS } from "./theaterFlightParts"

describe("the fillets' clock against the flight's", () => {
  it("lets any fillet fade finish well before the snap grows them from nothing", () => {
    // The snap cancels whatever fillet animation is left and grows from zero.
    // That is only invisible because no fade can still be showing by then.
    expect(
      FLIGHT_TRAVEL_HOLD_MS - FILLET_FADE_MS,
      "a fillet fade could still be showing when the snap restarts it from nothing, so the fillets would blink at the snap",
    ).toBeGreaterThanOrEqual(FILLET_FADE_MS / 2)
  })
})
