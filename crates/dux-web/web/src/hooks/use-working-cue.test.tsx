// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { fireAnimationStart } from "@/test/animationEvents"

import {
  useWorkingCue,
  WORKING_BOUNCE_ANIMATION,
  WORKING_PULSE_ANIMATION,
  WORKING_PULSE_FADE_MS,
} from "./use-working-cue"

// The bounce and the pulse live on two elements, a frame and the glyph inside
// it, so dropping the pulse is a style change on the glyph alone and can never
// restart, re-time or jump the bounce running on the frame.
function Probe({ working }: { working: boolean }) {
  const { bouncing, pulsing, attachBounce, attachPulse, handlers } = useWorkingCue<
    HTMLSpanElement,
    HTMLSpanElement
  >(working)
  return (
    <span
      ref={attachBounce}
      data-testid="frame"
      className={bouncing ? WORKING_BOUNCE_ANIMATION : "frame-rest"}
      {...handlers}
    >
      <span
        ref={attachPulse}
        data-testid="glyph"
        className={pulsing ? WORKING_PULSE_ANIMATION : "glyph-rest"}
      />
    </span>
  )
}

const frame = () => screen.getByTestId("frame")
const glyph = () => screen.getByTestId("glyph")
const bouncing = () => frame().className === WORKING_BOUNCE_ANIMATION
const pulsing = () => glyph().className === WORKING_PULSE_ANIMATION

// A stand-in for the browser's CSSAnimation, which jsdom does not implement.
// Writes to its clock are recorded, because rewinding or re-anchoring the
// bounce is exactly the jump a hand-over must never cause.
function fakeAnimation(animationName: string, currentIteration: number, startTime = 120) {
  let resolve!: () => void
  const finished = new Promise<void>((res) => {
    resolve = res
  })
  const updateTiming = vi.fn()
  const clockWrites: string[] = []
  const cancel = vi.fn()
  let start: number | null = startTime
  let current: number | null = 480
  const animation = {
    animationName,
    finished,
    cancel,
    effect: { getComputedTiming: () => ({ currentIteration }), updateTiming },
    get startTime() {
      return start
    },
    set startTime(value: number | null) {
      clockWrites.push(`startTime=${value}`)
      start = value
    },
    get currentTime() {
      return current
    },
    set currentTime(value: number | null) {
      clockWrites.push(`currentTime=${value}`)
      current = value
    },
  }
  return {
    animation,
    updateTiming,
    clockWrites,
    cancel,
    finish: async () => {
      await act(async () => resolve())
    },
  }
}

function withAnimations(el: Element, animations: () => unknown[]) {
  Object.defineProperty(el, "getAnimations", { configurable: true, value: animations })
}

function withAnimate(el: Element) {
  const fade = { cancel: vi.fn() }
  const animate = vi.fn(() => fade)
  Object.defineProperty(el, "animate", { configurable: true, value: animate })
  return { animate, fade }
}

function setHidden(hidden: boolean) {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden })
}

describe("useWorkingCue", () => {
  afterEach(() => {
    cleanup()
    setHidden(false)
    vi.restoreAllMocks()
  })

  it("bounces the frame and pulses the glyph while working", () => {
    render(<Probe working />)
    expect(bouncing()).toBe(true)
    expect(pulsing()).toBe(true)
  })

  it("rests both when it was never working", () => {
    render(<Probe working={false} />)
    expect(bouncing()).toBe(false)
    expect(pulsing()).toBe(false)
  })

  // Every stop, a plain one or a higher state taking over, is the same stop:
  // the pulse yields at once and the bounce in flight finishes at rest.
  it("drops the pulse at once and finishes the bounce in flight, untouched", async () => {
    const bounce = fakeAnimation("working-bounce", 5)
    const pulse = fakeAnimation("working-pulse", 1)
    const { rerender } = render(<Probe working />)
    withAnimations(frame(), () => [bounce.animation])
    withAnimations(glyph(), () => [pulse.animation])
    withAnimate(glyph())
    fireAnimationStart(frame(), "working-bounce")
    const frameClass = frame().className

    rerender(<Probe working={false} />)
    // The pulse is gone before anything is painted.
    expect(pulsing()).toBe(false)
    // The frame's style did not change at all, so nothing can restart the
    // bounce: the same animation is told to end with its current iteration,
    // and its clock is never rewound or re-anchored.
    expect(frame().className).toBe(frameClass)
    expect(bounce.updateTiming).toHaveBeenCalledTimes(1)
    expect(bounce.updateTiming).toHaveBeenCalledWith({ iterations: 6 })
    expect(bounce.clockWrites).toEqual([])
    expect(bounce.animation.currentTime).toBe(480)
    expect(bounce.cancel).not.toHaveBeenCalled()

    await bounce.finish()
    expect(bouncing()).toBe(false)
  })

  // Removing a CSS animation does not start a CSS transition from its
  // animated value (measured in Chromium: the opacity snaps to 1), so the
  // glyph eases back to full from wherever the pulse left it, explicitly.
  it("eases the glyph from the pulse's current opacity back to full", () => {
    const pulse = fakeAnimation("working-pulse", 0)
    const { rerender } = render(<Probe working />)
    withAnimations(glyph(), () => [pulse.animation])
    const { animate } = withAnimate(glyph())
    const real = window.getComputedStyle
    vi.spyOn(window, "getComputedStyle").mockImplementation((el, pseudo) =>
      el === glyph() ? ({ opacity: "0.55" } as CSSStyleDeclaration) : real(el, pseudo),
    )

    rerender(<Probe working={false} />)
    expect(animate).toHaveBeenCalledTimes(1)
    expect(animate).toHaveBeenCalledWith(
      [{ opacity: "0.55" }, { opacity: "1" }],
      { duration: WORKING_PULSE_FADE_MS, easing: "ease-out" },
    )
    expect(pulsing()).toBe(false)
  })

  // Reduced motion never started the pulse, and a hidden tab paints nothing:
  // there is no dip to ease out of.
  it("eases nothing when no pulse is running", () => {
    const { rerender } = render(<Probe working />)
    withAnimations(glyph(), () => [])
    const { animate } = withAnimate(glyph())
    rerender(<Probe working={false} />)
    expect(animate).not.toHaveBeenCalled()
    expect(pulsing()).toBe(false)
  })

  it("eases nothing in a hidden tab", () => {
    const pulse = fakeAnimation("working-pulse", 0)
    const { rerender } = render(<Probe working />)
    withAnimations(glyph(), () => [pulse.animation])
    const { animate } = withAnimate(glyph())
    setHidden(true)
    rerender(<Probe working={false} />)
    expect(animate).not.toHaveBeenCalled()
    expect(pulsing()).toBe(false)
  })

  // Work resuming while the last bounce is still in flight restarts the pulse
  // but not the bounce, so the new pulse is anchored to the bounce's own start:
  // four bounces per pulse, every pulse boundary a bounce boundary.
  it("re-anchors a resumed pulse to the bounce still in flight", () => {
    const bounce = fakeAnimation("working-bounce", 3, 1000)
    const oldPulse = fakeAnimation("working-pulse", 0, 1000)
    const newPulse = fakeAnimation("working-pulse", 0, 2345)
    let pulses = [oldPulse.animation]
    const { rerender } = render(<Probe working />)
    withAnimations(frame(), () => [bounce.animation])
    withAnimations(glyph(), () => pulses)
    const { fade } = withAnimate(glyph())
    fireAnimationStart(frame(), "working-bounce")

    rerender(<Probe working={false} />)
    pulses = [newPulse.animation]
    rerender(<Probe working />)
    expect(pulsing()).toBe(true)
    expect(bouncing()).toBe(true)
    expect(newPulse.animation.startTime).toBe(1000)
    // The ease back from the stop does not fight the resumed pulse.
    expect(fade.cancel).toHaveBeenCalled()
    // The bounce itself is never re-anchored.
    expect(bounce.clockWrites).toEqual([])
  })

  it("leaves a fresh start alone, where both begin on the same frame", () => {
    const bounce = fakeAnimation("working-bounce", 0, 50)
    const pulse = fakeAnimation("working-pulse", 0, 50)
    const { rerender } = render(<Probe working={false} />)
    withAnimations(frame(), () => [bounce.animation])
    withAnimations(glyph(), () => [pulse.animation])
    rerender(<Probe working />)
    expect(pulse.clockWrites).toEqual([])
  })
})
