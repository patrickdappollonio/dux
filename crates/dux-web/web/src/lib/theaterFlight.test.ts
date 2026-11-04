import { describe, expect, it } from "vitest"

import {
  FLIGHT_ATTACH_HOLD_MS,
  FLIGHT_CHROME_SLACK_MS,
  FLIGHT_EASE,
  FLIGHT_TRAVEL_HOLD_MS,
  FLIGHT_TRAVEL_MS,
  flapMounted,
  flapVisible,
  flightEaseProgress,
  flightForMode,
  flightForModeFrom,
  flightHoldMs,
  flightNext,
  flightOffset,
  flightOwnsPosition,
  flightTranslation,
  pillMounted,
  transparentShadow,
  type FlightPhase,
} from "@/lib/theaterFlight"

const CHROME_MS = 300

/// Walk the machine from a resting state until it rests again, collecting the
/// stages, so the ORDER is asserted as a sequence rather than one arrow at a
/// time.
function walk(from: FlightPhase): FlightPhase[] {
  const seen: FlightPhase[] = [from]
  let phase = from
  for (let i = 0; i < 10; i++) {
    const next = flightNext(phase)
    if (next === phase) break
    seen.push(next)
    phase = next
  }
  return seen
}

describe("the phone's theater flight, stage by stage", () => {
  it("goes chrome first, then the flight, entering", () => {
    expect(flightForMode(true, CHROME_MS)).toBe("collapsing")
    expect(walk("collapsing")).toEqual(["collapsing", "detaching", "floating"])
  })

  it("goes chrome first, then the flight, then the snap, leaving", () => {
    expect(flightForMode(false, CHROME_MS)).toBe("expanding")
    expect(walk("expanding")).toEqual([
      "expanding",
      "returning",
      "attaching",
      "docked",
    ])
  })

  it("undoes a chrome stage the mode flipped back out of, with no flight", () => {
    // Nothing has moved yet at either chrome stage, so the gesture that was
    // abandoned rests where it started. Running the opposite flight from there
    // would hide a flap that never left the band and float a pill over a screen
    // the cluster never flew off.
    expect(flightForModeFrom("collapsing", false, CHROME_MS)).toBe("docked")
    expect(flightForModeFrom("expanding", true, CHROME_MS)).toBe("floating")
  })

  it("turns a cluster that is in the air around where it is", () => {
    // Mid-flight the cluster is the pill, somewhere between the two ends, so
    // the gesture the other way is the flight the other way, starting now:
    // neither chrome stage, which would put the flap back on the band first or
    // leave the pill flying away from the dock it was just asked back to.
    expect(flightForModeFrom("detaching", false, CHROME_MS)).toBe("returning")
    expect(flightForModeFrom("returning", true, CHROME_MS)).toBe("detaching")
    expect(flightForModeFrom("attaching", true, CHROME_MS)).toBe("detaching")
  })

  it("starts a whole gesture from either resting state", () => {
    expect(flightForModeFrom("floating", false, CHROME_MS)).toBe("expanding")
    expect(flightForModeFrom("docked", true, CHROME_MS)).toBe("collapsing")
  })

  it("rests rather than sticking when an instant swap lands mid-flight", () => {
    // A cover arriving passes ms 0 (there is nothing to fly to under one), and
    // it can land on a stage that really is in the air. Every answer is a
    // RESTING phase, so the cluster is painted at one end or the other rather
    // than held halfway with a timer nobody armed.
    expect(flightForModeFrom("detaching", false, 0)).toBe("docked")
    expect(flightForModeFrom("returning", true, 0)).toBe("floating")
    expect(flightForModeFrom("attaching", false, 0)).toBe("docked")
  })

  it("still cuts straight to the resting state for reduced motion", () => {
    expect(flightForModeFrom("collapsing", true, 0)).toBe("floating")
    expect(flightForModeFrom("docked", true, 0)).toBe("floating")
  })

  it("collapses every stage to an instant swap for reduced motion", () => {
    expect(flightForMode(true, 0)).toBe("floating")
    expect(flightForMode(false, 0)).toBe("docked")
    expect(flightHoldMs("floating", 0)).toBeNull()
    expect(flightHoldMs("docked", 0)).toBeNull()
  })

  it("gives the two chrome stages the CHROME's clock, not one of their own", () => {
    // The flap may not leave until the band has, and the capsule may not fly
    // home until the dock is back on screen. Plus the slack the transition
    // starts a paint late by: the stage that follows MEASURES that dock, and a
    // handover exactly on the duration reads it a few pixels short.
    expect(flightHoldMs("collapsing", CHROME_MS)).toBe(
      CHROME_MS + FLIGHT_CHROME_SLACK_MS,
    )
    expect(flightHoldMs("expanding", CHROME_MS)).toBe(
      CHROME_MS + FLIGHT_CHROME_SLACK_MS,
    )
    expect(FLIGHT_CHROME_SLACK_MS).toBeGreaterThan(0)
  })

  it("holds each travel a frame past its own transition", () => {
    expect(FLIGHT_TRAVEL_HOLD_MS).toBeGreaterThan(FLIGHT_TRAVEL_MS)
    expect(flightHoldMs("detaching", CHROME_MS)).toBe(FLIGHT_TRAVEL_HOLD_MS)
    expect(flightHoldMs("returning", CHROME_MS)).toBe(FLIGHT_TRAVEL_HOLD_MS)
    expect(flightHoldMs("attaching", CHROME_MS)).toBe(FLIGHT_ATTACH_HOLD_MS)
  })
})

describe("what exists at each stage", () => {
  it("keeps the flap mounted everywhere theater is not settled", () => {
    const phases: FlightPhase[] = [
      "docked",
      "collapsing",
      "detaching",
      "expanding",
      "returning",
      "attaching",
    ]
    for (const phase of phases) expect(flapMounted(phase)).toBe(true)
    expect(flapMounted("floating")).toBe(false)
  })

  it("paints the flap only while it is really the cluster on screen", () => {
    expect(flapVisible("docked")).toBe(true)
    expect(flapVisible("collapsing")).toBe(true)
    // Mounted but not painted: through the return it is the dock being
    // measured, and a visible flap under a capsule flying onto it is two
    // clusters at once.
    for (const phase of ["detaching", "expanding", "returning", "attaching"] as const) {
      expect(flapVisible(phase)).toBe(false)
    }
  })

  it("mounts the pill from pull-off until the snap is over", () => {
    expect(pillMounted("docked")).toBe(false)
    expect(pillMounted("collapsing")).toBe(false)
    for (const phase of ["detaching", "floating", "expanding", "returning", "attaching"] as const) {
      expect(pillMounted(phase)).toBe(true)
    }
  })

  it("hands the pill's coordinates to the flight only on the way home", () => {
    // The detach flies to a dock the pill placed itself, so it keeps its own
    // position and the flight only adds a transform.
    expect(flightOwnsPosition("detaching")).toBe(false)
    expect(flightOwnsPosition("floating")).toBe(false)
    expect(flightOwnsPosition("returning")).toBe(true)
    expect(flightOwnsPosition("attaching")).toBe(true)
  })

  it("never has both clusters painted at once", () => {
    const phases: FlightPhase[] = [
      "docked",
      "collapsing",
      "detaching",
      "floating",
      "expanding",
      "returning",
      "attaching",
    ]
    for (const phase of phases) {
      expect(flapVisible(phase) && pillMounted(phase)).toBe(false)
    }
  })
})

describe("the flight's arithmetic", () => {
  it("translates from one painted box to another, and nothing else", () => {
    expect(
      flightTranslation({ left: 240, top: 44 }, { left: 200, top: 620 }),
    ).toEqual({ x: 40, y: -576 })
  })

  it("is its own inverse, which is what makes the return land on the dock", () => {
    const a = { left: 12, top: 7 }
    const b = { left: 300, top: 500 }
    const out = flightTranslation(a, b)
    const back = flightTranslation(b, a)
    expect(out.x).toBe(-back.x)
    expect(out.y).toBe(-back.y)
  })

  it("puts a viewport rectangle into its offset parent's space", () => {
    expect(
      flightOffset({ left: 260, top: 130 }, { left: 20, top: 100 }),
    ).toEqual({ left: 240, top: 30 })
  })
})

describe("the shadow that fades rather than pops", () => {
  it("zeroes every colour and keeps every offset", () => {
    expect(
      transparentShadow(
        "rgba(0, 0, 0, 0.1) 0px 10px 15px -3px, rgba(0, 0, 0, 0.1) 0px 4px 6px -4px",
      ),
    ).toBe(
      "rgba(0, 0, 0, 0) 0px 10px 15px -3px, rgba(0, 0, 0, 0) 0px 4px 6px -4px",
    )
  })

  it("handles the modern colour syntaxes a browser may report", () => {
    expect(transparentShadow("oklch(0.2 0 0 / 55%) 0 10px 30px")).toBe(
      "rgba(0, 0, 0, 0) 0 10px 30px",
    )
    expect(transparentShadow("rgb(0 0 0 / 40%) 0 0 0 1px")).toBe(
      "rgba(0, 0, 0, 0) 0 0 0 1px",
    )
  })

  it("refuses to invent a value it was never given", () => {
    // A shadow going to or from `none` snaps, and so does one built out of a
    // guess. The caller skips the fade instead.
    expect(transparentShadow("none")).toBeNull()
    expect(transparentShadow("")).toBeNull()
    expect(transparentShadow(null)).toBeNull()
    expect(transparentShadow(undefined)).toBeNull()
    expect(transparentShadow("0 10px 30px")).toBeNull()
  })

  it("is stable across calls, so a second flight fades like the first", () => {
    const shadow = "rgba(0, 0, 0, 0.5) 0 1px 2px"
    expect(transparentShadow(shadow)).toBe(transparentShadow(shadow))
  })
})

describe("the flight's easing, as the browser runs it", () => {
  it("names the same curve it computes", () => {
    expect(FLIGHT_EASE).toBe("cubic-bezier(0.2, 0, 0, 1)")
  })

  it("starts at rest, ends at rest and never runs backwards", () => {
    expect(flightEaseProgress(0)).toBe(0)
    expect(flightEaseProgress(1)).toBe(1)
    let last = 0
    for (let i = 1; i <= 100; i++) {
      const p = flightEaseProgress(i / 100)
      expect(p).toBeGreaterThanOrEqual(last)
      last = p
    }
  })

  it("decelerates: most of the distance is covered in the first half of the time", () => {
    // A known point on cubic-bezier(0.2, 0, 0, 1): the parameter 0.5 lands at
    // x = 0.075 + 0.125 = 0.2, y = 0.375 + 0.125 = 0.5.
    expect(flightEaseProgress(0.2)).toBeCloseTo(0.5, 6)
    expect(flightEaseProgress(0.5)).toBeGreaterThan(0.85)
  })
})
