// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"

import { guardOpeningTouch, touchIsDown, trackTouches } from "./openingTouch"

// jsdom has no touch stack, so these fire the events a browser would and pin
// the rule: the touch that opened a card chooses nothing from it, and the first
// input that begins after it is a deliberate one. What a real iPhone or Android
// does with its own long press is the part only a device can answer.

function touch(type: "touchstart" | "touchend" | "touchcancel", target: EventTarget, touches: number) {
  const event = new Event(type, { bubbles: true, cancelable: true })
  Object.defineProperty(event, "touches", { value: new Array(touches).fill({}) })
  target.dispatchEvent(event)
  return event
}

function press(target: EventTarget, type: string) {
  const event = new Event(type, { bubbles: true, cancelable: true })
  target.dispatchEvent(event)
  return event
}

trackTouches()

let release: () => void = () => {}
afterEach(() => {
  release()
  release = () => {}
  touch("touchend", document, 0)
  document.body.innerHTML = ""
})

function card() {
  const item = document.createElement("div")
  const onClick = vi.fn()
  item.addEventListener("click", onClick)
  document.body.append(item)
  return { item, onClick }
}

describe("the touch that opened a card", () => {
  it("knows whether a finger is down", () => {
    expect(touchIsDown()).toBe(false)
    touch("touchstart", document.body, 1)
    expect(touchIsDown()).toBe(true)
    touch("touchend", document.body, 0)
    expect(touchIsDown()).toBe(false)
  })

  it("chooses nothing when it lifts: its release is cancelled and a click that follows is swallowed", () => {
    const { item, onClick } = card()
    touch("touchstart", document.body, 1)
    release = guardOpeningTouch()
    const lift = touch("touchend", document.body, 0)
    expect(lift.defaultPrevented).toBe(true)
    // A browser that sends the click anyway, and the mouse events before it.
    press(item, "mouseup")
    press(item, "click")
    expect(onClick).not.toHaveBeenCalled()
  })

  it("lets a later, deliberate tap choose", () => {
    const { item, onClick } = card()
    touch("touchstart", document.body, 1)
    release = guardOpeningTouch()
    touch("touchend", document.body, 0)
    press(item, "click")
    expect(onClick).not.toHaveBeenCalled()

    touch("touchstart", item, 1)
    const lift = touch("touchend", item, 0)
    expect(lift.defaultPrevented).toBe(false)
    press(item, "click")
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it.each(["pointerdown", "keydown"])("lets a click after a %s choose", (type) => {
    const { item, onClick } = card()
    touch("touchstart", document.body, 1)
    release = guardOpeningTouch()
    touch("touchend", document.body, 0)
    press(item, type)
    press(item, "click")
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it("is not armed for a card opened with no finger down, so a mouse and the keyboard are untouched", () => {
    const { item, onClick } = card()
    release = guardOpeningTouch()
    press(item, "click")
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it("stops guarding once the card is gone", () => {
    const { item, onClick } = card()
    touch("touchstart", document.body, 1)
    release = guardOpeningTouch()
    release()
    touch("touchend", document.body, 0)
    press(item, "click")
    expect(onClick).toHaveBeenCalledTimes(1)
  })
})
