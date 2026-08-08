import { describe, expect, it } from "vitest"

import {
  MOUSE_DRAG_ACTIVATION,
  TOUCH_DRAG_ACTIVATION,
} from "./dragActivation"

// The two reorder-drag activation constraints (agents list and terminals list
// share them through the one `useSensors` in FlatAgentList). Mouse keeps the
// small distance gate (a plain click stays a select); touch arms on a HOLD,
// because an instantly-arming touch drag fights the list's own scroll gesture
// and aborts, which is the "briefly glitches, never reorders" phone bug.
describe("reorder drag activation constraints", () => {
  it("mouse arms on a short distance so a click never drags", () => {
    expect(MOUSE_DRAG_ACTIVATION.distance).toBe(6)
  })

  it("touch arms on a hold long enough to be deliberate but shorter than a long-press", () => {
    // Below ~200ms a scroll-intent touch still arms the drag; at ~500ms the
    // browser's own long-press behaviors (context menu, text selection)
    // start competing with the hold.
    expect(TOUCH_DRAG_ACTIVATION.delay).toBeGreaterThanOrEqual(200)
    expect(TOUCH_DRAG_ACTIVATION.delay).toBeLessThan(500)
    // The tolerance is what lets a finger that starts SCROLLING during the
    // hold cancel the drag: moving further than this before the delay
    // elapses aborts activation and the list scrolls normally.
    expect(TOUCH_DRAG_ACTIVATION.tolerance).toBeGreaterThan(0)
    expect(TOUCH_DRAG_ACTIVATION.tolerance).toBeLessThanOrEqual(10)
  })
})
