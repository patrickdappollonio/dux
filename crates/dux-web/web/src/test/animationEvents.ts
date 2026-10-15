import { act } from "@testing-library/react"
import { expect, vi } from "vitest"

// jsdom ships no `AnimationEvent`, which changes two things Testing Library's
// `fireEvent.animation*` gets wrong for a React handler: the event falls back
// to a plain `Event` with no `animationName`, the one field a handler keys on,
// and React, seeing no `AnimationEvent`, listens for the WebKit-prefixed event
// names instead of the standard ones. These fire what React is listening for,
// with the name attached, playing the part of the browser reporting a CSS
// animation's start and cycle boundaries.
const STANDARD = {
  start: "animationstart",
  iteration: "animationiteration",
  end: "animationend",
} as const
const PREFIXED = {
  start: "webkitAnimationStart",
  iteration: "webkitAnimationIteration",
  end: "webkitAnimationEnd",
} as const

function fireAnimation(kind: keyof typeof STANDARD, el: Element, animationName: string) {
  const type = "AnimationEvent" in window ? STANDARD[kind] : PREFIXED[kind]
  const event = new Event(type, { bubbles: true })
  Object.defineProperty(event, "animationName", { value: animationName })
  act(() => {
    el.dispatchEvent(event)
  })
}

export const fireAnimationStart = (el: Element, animationName: string) =>
  fireAnimation("start", el, animationName)
export const fireAnimationIteration = (el: Element, animationName: string) =>
  fireAnimation("iteration", el, animationName)
export const fireAnimationEnd = (el: Element, animationName: string) =>
  fireAnimation("end", el, animationName)

// React has no handler prop for `animationcancel`, so the hook listens for it
// natively under its one standard name.
export function fireAnimationCancel(el: Element, animationName: string) {
  const event = new Event("animationcancel", { bubbles: true })
  Object.defineProperty(event, "animationName", { value: animationName })
  act(() => {
    el.dispatchEvent(event)
  })
}

// The landed look's timing, read off the frame's classes because jsdom has no
// stylesheet: the color must ease over a real duration, and the blink must be
// delayed by at least that long, with `!` so the blink's own `animation`
// shorthand cannot reset the delay to zero.
export function expectLookEasesIn(className: string, message: string) {
  const colorMs = Number(/(?:^|\s)duration-(\d+)(?=\s|$)/.exec(className)?.[1])
  const blinkDelayMs = Number(/(?:^|\s)\[animation-delay:(\d+)ms\]!(?=\s|$)/.exec(className)?.[1])
  expect(className, message).toMatch(/(?:^|\s)transition-colors(?=\s|$)/)
  expect(colorMs, message).toBeGreaterThanOrEqual(150)
  expect(blinkDelayMs, message).toBeGreaterThanOrEqual(colorMs)
}

// Plays the browser's part in a stopped glyph easing back to full opacity:
// jsdom has neither `getAnimations` nor `animate`, so this glyph reports a
// running pulse and records the ease it is asked for. `eased` resolves that
// ease, the moment the glyph is back at full opacity, and a cancel rejects it,
// as the browser's does.
export function stubGlyphEase(glyph: Element) {
  let finish!: () => void
  let abort!: () => void
  const finished = new Promise<void>((resolve, reject) => {
    finish = resolve
    abort = () => reject(new DOMException("cancelled", "AbortError"))
  })
  const animate = vi.fn(() => ({ cancel: vi.fn(abort), finished }))
  Object.defineProperty(glyph, "getAnimations", {
    configurable: true,
    value: () => [{ animationName: "working-pulse" }],
  })
  Object.defineProperty(glyph, "animate", { configurable: true, value: animate })
  return {
    animate,
    eased: async () => {
      await act(async () => finish())
    },
  }
}
