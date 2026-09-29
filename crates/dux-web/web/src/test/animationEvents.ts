import { act } from "@testing-library/react"

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
