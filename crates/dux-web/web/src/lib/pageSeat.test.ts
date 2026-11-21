import { beforeEach, describe, expect, it, vi } from "vitest"

type Seat = typeof import("./pageSeat")

const OPENER = "b".repeat(32)
let seat: Seat

beforeEach(async () => {
  vi.resetModules()
  seat = await import("./pageSeat")
})

describe("the pages a tab acts as", () => {
  it("names its opener only while it is the standalone editor", () => {
    const own = seat.pageSeatId()
    // An ordinary app tab is handed nothing, whoever offers.
    expect(seat.adoptOpenerPages([OPENER])).toBe(false)
    expect(seat.actingPageIds()).toEqual([own])

    seat.setEditorSurface(true)
    expect(seat.adoptOpenerPages([OPENER])).toBe(true)
    expect(seat.actingPageIds()).toEqual([own, OPENER])

    // The tab became the ordinary app: it acts as itself alone from here on,
    // and coming back to an editor address does not bring the old answer back.
    seat.setEditorSurface(false)
    expect(seat.actsForOpener()).toBe(false)
    expect(seat.actingPageIds()).toEqual([own])
    seat.setEditorSurface(true)
    expect(seat.actingPageIds()).toEqual([own])
  })
})
