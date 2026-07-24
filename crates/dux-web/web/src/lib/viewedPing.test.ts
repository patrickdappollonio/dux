import { describe, expect, it } from "vitest"
import {
  VIEWED_PING_INTERVAL_MS,
  shouldSendViewed,
  visibleSinceAfterTransition,
  withinAttentionGrace,
} from "./viewedPing"

describe("shouldSendViewed", () => {
  it("pings only when this device is the owner AND visible", () => {
    expect(shouldSendViewed({ isOwner: true, visible: true })).toBe(true)
    // A read-only observer must not suppress attention for everyone.
    expect(shouldSendViewed({ isOwner: false, visible: true })).toBe(false)
    // A backgrounded owner's open socket must not read as "watching".
    expect(shouldSendViewed({ isOwner: true, visible: false })).toBe(false)
    expect(shouldSendViewed({ isOwner: false, visible: false })).toBe(false)
  })

  it("pings comfortably faster than the 3s engagement window", () => {
    expect(VIEWED_PING_INTERVAL_MS).toBeLessThan(3000)
  })

  it("with no grace context, behaves exactly as before (backward compatible)", () => {
    expect(shouldSendViewed({ isOwner: true, visible: true })).toBe(true)
    expect(shouldSendViewed({ isOwner: false, visible: true })).toBe(false)
  })

  it("suppresses pings while within the attention grace window after a transition", () => {
    expect(
      shouldSendViewed({
        isOwner: true,
        visible: true,
        now: 1_000,
        visibleSince: 500,
        graceMs: 3000,
      }),
    ).toBe(false)
  })

  it("allows pings once the grace window has elapsed", () => {
    expect(
      shouldSendViewed({
        isOwner: true,
        visible: true,
        now: 3_500,
        visibleSince: 500,
        graceMs: 3000,
      }),
    ).toBe(true)
  })

  it("still requires ownership and visibility even outside the grace window", () => {
    expect(
      shouldSendViewed({
        isOwner: false,
        visible: true,
        now: 5_000,
        visibleSince: 500,
        graceMs: 3000,
      }),
    ).toBe(false)
    expect(
      shouldSendViewed({
        isOwner: true,
        visible: false,
        now: 5_000,
        visibleSince: 500,
        graceMs: 3000,
      }),
    ).toBe(false)
  })
})

// SHARED VECTORS with dux-core `focus.rs` `within_attention_grace_semantics`:
// undefined-since -> false, grace<=0 -> false, elapsed<grace -> true, boundary
// exclusive. Mirrored there so the grace math cannot drift between surfaces.
describe("withinAttentionGrace", () => {
  it("is false with no visibleSince (initial load has no grace)", () => {
    expect(withinAttentionGrace(1000, undefined, 3000)).toBe(false)
  })

  it("is true right at the transition instant", () => {
    expect(withinAttentionGrace(500, 500, 3000)).toBe(true)
  })

  it("is true just before the grace boundary", () => {
    expect(withinAttentionGrace(3499, 500, 3000)).toBe(true)
  })

  it("is false exactly at and after the grace boundary", () => {
    expect(withinAttentionGrace(3500, 500, 3000)).toBe(false)
    expect(withinAttentionGrace(4000, 500, 3000)).toBe(false)
  })

  it("graceMs 0 never suppresses", () => {
    expect(withinAttentionGrace(500, 500, 0)).toBe(false)
    expect(withinAttentionGrace(500.0001, 500, 0)).toBe(false)
  })
})

describe("visibleSinceAfterTransition", () => {
  it("returns undefined on initial load while visible (no observed transition)", () => {
    // prevVisible is undefined the first time this runs (no prior sample).
    expect(
      visibleSinceAfterTransition(undefined, true, undefined, 1000),
    ).toBeUndefined()
  })

  it("arms the grace on a real hidden -> visible transition", () => {
    expect(visibleSinceAfterTransition(false, true, undefined, 2000)).toBe(
      2000,
    )
  })

  it("does not re-arm on a redundant visible -> visible signal (e.g. a focus event)", () => {
    expect(visibleSinceAfterTransition(true, true, 2000, 5000)).toBe(2000)
  })

  it("resets to undefined when the document goes hidden again", () => {
    expect(visibleSinceAfterTransition(true, false, 2000, 5000)).toBeUndefined()
    expect(
      visibleSinceAfterTransition(false, false, undefined, 5000),
    ).toBeUndefined()
  })
})
