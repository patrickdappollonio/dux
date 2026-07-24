import { describe, expect, it } from "vitest"

import {
  dragScrollLines,
  dragWheelReport,
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

describe("dragWheelReport", () => {
  // A finger drag forwarded to a mouse-tracking alt-screen app (Claude Code,
  // Codex, ...) must send at most ONE wheel notch per touch-move event, matching
  // the desktop path where xterm forwards exactly one report per physical wheel
  // event (1:1 per tick). The unbounded `dragScrollLines` magnitude, emitted as a
  // dense burst of SGR reports in a single WS frame, is what corrupted the app's
  // scrollback-pager repaint on a fast flick.
  it("does not forward for a sub-row drag, carrying the remainder", () => {
    expect(dragWheelReport(10, 16)).toEqual({ notch: 0, remainderPx: 10 })
  })

  it("forwards ONE wheel-up notch for a downward one-row drag (older output)", () => {
    expect(dragWheelReport(20, 16)).toEqual({ notch: -1, remainderPx: 4 })
  })

  it("forwards ONE wheel-down notch for an upward one-row drag (newer output)", () => {
    expect(dragWheelReport(-20, 16)).toEqual({ notch: 1, remainderPx: -4 })
  })

  it("CAPS a fast flick to a single notch while draining the whole accumulator", () => {
    // A fast flick accumulates many rows in one move: `dragScrollLines` would
    // emit a 12-report burst here (proven below), the pathological input that
    // glitches the app's repaint. `dragWheelReport` caps the forwarded notch to
    // magnitude 1 yet consumes the same whole rows, so the accumulator never
    // grows and the drag still tracks the finger across successive moves.
    expect(dragScrollLines(200, 16)).toEqual({
      scrollLines: -12,
      remainderPx: 8,
    })
    expect(dragWheelReport(200, 16)).toEqual({ notch: -1, remainderPx: 8 })
    expect(dragWheelReport(-200, 16)).toEqual({ notch: 1, remainderPx: -8 })
  })

  it("carries no scroll for zero movement", () => {
    expect(dragWheelReport(0, 16)).toEqual({ notch: 0, remainderPx: 0 })
  })
})
