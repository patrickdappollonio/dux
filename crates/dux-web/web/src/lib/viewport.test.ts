import { describe, expect, it } from "vitest"

import { KEYBOARD_OPEN_THRESHOLD_PX, keyboardLikelyOpen } from "./viewport"

describe("keyboardLikelyOpen", () => {
  it("is false when the visual and layout viewports match (no keyboard)", () => {
    expect(keyboardLikelyOpen(800, 800)).toBe(false)
  })

  it("is true when the visual viewport is much shorter (keyboard up)", () => {
    expect(keyboardLikelyOpen(500, 800)).toBe(true)
  })

  it("ignores a small delta like the iOS URL-bar collapse", () => {
    expect(keyboardLikelyOpen(800 - 90, 800)).toBe(false)
  })

  it("treats a delta just over the threshold as open", () => {
    expect(
      keyboardLikelyOpen(800 - (KEYBOARD_OPEN_THRESHOLD_PX + 1), 800),
    ).toBe(true)
  })

  it("treats a delta exactly at the threshold as closed (strict >)", () => {
    expect(keyboardLikelyOpen(800 - KEYBOARD_OPEN_THRESHOLD_PX, 800)).toBe(
      false,
    )
  })
})
