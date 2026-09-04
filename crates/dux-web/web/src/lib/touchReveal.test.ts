import { describe, expect, it } from "vitest"

import { ALWAYS_REVEALED_ON_TOUCH } from "./touchReveal"

describe("ALWAYS_REVEALED_ON_TOUCH", () => {
  // Pinned literally, because every hover-revealed ⋯ wrapper depends on both
  // halves being present: the width one gives the trigger its slot back and the
  // opacity one makes it visible in it. Losing either leaves a finger with a
  // control it can see but not press, or press but not see.
  it("is the pointer-coarse override for both the slot and the paint", () => {
    expect(ALWAYS_REVEALED_ON_TOUCH).toBe(
      "pointer-coarse:max-w-none pointer-coarse:opacity-100",
    )
  })

  // The whole point of the fix: this is a question about the POINTER, and
  // answering it with the viewport-width breakpoint is what left a landscape
  // tablet (desktop layout, finger for a pointer) unable to reach the menu.
  it("asks about the pointer rather than the viewport width", () => {
    expect(ALWAYS_REVEALED_ON_TOUCH).not.toMatch(/\bmd:|max-md:/)
  })
})
