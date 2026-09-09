import { describe, expect, it } from "vitest"

import { PILL_GRIPLESS_CLASS } from "@/lib/theaterPill"

import { theaterPillBox } from "./theaterPillBox"

const box = (over: Partial<Parameters<typeof theaterPillBox>[0]> = {}) =>
  theaterPillBox({
    flight: null,
    position: { x: 12, y: 34 },
    dragging: false,
    justDropped: false,
    reducedMotion: false,
    gripless: false,
    ...over,
  })

describe("theaterPillBox", () => {
  it("places a measured pill by coordinate and an unmeasured one in the corner", () => {
    expect(box().style).toEqual({ left: 12, top: 34 })
    expect(box().className).not.toContain("right-3.5")

    const unmeasured = box({ position: null })
    expect(unmeasured.style).toBeUndefined()
    expect(unmeasured.className).toContain("right-3.5")
    expect(unmeasured.className).toContain("bottom-3.5")
  })

  it("hands the coordinates to the flight only on the stages that fly home", () => {
    expect(box({ flight: "returning" }).style).toBeUndefined()
    expect(box({ flight: "attaching" }).style).toBeUndefined()
    // The detach flies out from where the pill already is, so the box keeps its
    // own coordinates; the chrome stages leave a resting pill alone.
    expect(box({ flight: "detaching" }).style).toEqual({ left: 12, top: 34 })
    expect(box({ flight: "expanding" }).style).toEqual({ left: 12, top: 34 })
  })

  it("settles only when nothing else is moving the box", () => {
    const settles = (over: Partial<Parameters<typeof theaterPillBox>[0]>) =>
      box(over).className.includes("transition-[left,top]")
    expect(settles({})).toBe(true)
    expect(settles({ dragging: true })).toBe(false)
    expect(settles({ justDropped: true })).toBe(false)
    expect(settles({ reducedMotion: true })).toBe(false)
    // The re-clamp under returning chrome snaps rather than crawling upward.
    expect(settles({ flight: "expanding" })).toBe(false)
    expect(settles({ flight: "returning" })).toBe(false)
  })

  it("names the stage it is in, and only that stage", () => {
    expect(box({ flight: "detaching" }).className).toContain("dux-flight-out")
    expect(box({ flight: "returning" }).className).toContain("dux-flight-in")
    expect(box({ flight: "attaching" }).className).toContain("dux-flight-attach")
    expect(box().className).not.toContain("dux-flight-")
  })

  it("collapses the grip slot on request", () => {
    expect(box({ gripless: true }).className).toContain(PILL_GRIPLESS_CLASS)
    expect(box().className).not.toContain(PILL_GRIPLESS_CLASS)
  })
})
