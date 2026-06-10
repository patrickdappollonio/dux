import { describe, expect, it } from "vitest"

import {
  dragScrollLines,
  KEYBOARD_OPEN_THRESHOLD_PX,
  keyboardLikelyOpen,
} from "./viewport"

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

describe("dragScrollLines", () => {
  it("does not scroll for no movement", () => {
    expect(dragScrollLines(0, 16)).toEqual({ scrollLines: 0, remainderPx: 0 })
  })

  it("does not scroll for a sub-row drag, carrying the remainder", () => {
    expect(dragScrollLines(10, 16)).toEqual({ scrollLines: 0, remainderPx: 10 })
  })

  it("dragging DOWN scrolls toward OLDER output (negative scrollLines)", () => {
    expect(dragScrollLines(20, 16)).toEqual({ scrollLines: -1, remainderPx: 4 })
  })

  it("dragging UP scrolls toward NEWER output (positive scrollLines)", () => {
    expect(dragScrollLines(-20, 16)).toEqual({ scrollLines: 1, remainderPx: -4 })
  })

  it("scrolls multiple lines at once and keeps the sub-row remainder", () => {
    expect(dragScrollLines(35, 16)).toEqual({ scrollLines: -2, remainderPx: 3 })
  })

  it("scrolls exact row multiples with no remainder (fencepost)", () => {
    expect(dragScrollLines(16, 16)).toEqual({ scrollLines: -1, remainderPx: 0 })
    expect(dragScrollLines(-32, 16)).toEqual({ scrollLines: 2, remainderPx: 0 })
  })

  it("falls back to a safe row height when given zero (no divide-by-zero)", () => {
    expect(dragScrollLines(16, 0)).toEqual({ scrollLines: -1, remainderPx: 0 })
  })

  it("falls back to a safe row height when given a negative height", () => {
    expect(dragScrollLines(32, -5)).toEqual({ scrollLines: -2, remainderPx: 0 })
  })
})
