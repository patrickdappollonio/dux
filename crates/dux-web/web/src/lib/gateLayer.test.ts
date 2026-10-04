// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { GATE_LAYER_ATTR, isInGateLayer, setGateUp } from "./gateLayer"

// The capture shield. The hidden app listens on the document in the CAPTURE
// phase (the divider drags, the terminal's composition tracking, the panel
// library), which a stop at the gate layer itself comes too late for. A window
// capture listener runs before every one of them.

let host: HTMLDivElement
let field: HTMLInputElement
let outside: HTMLDivElement

beforeEach(() => {
  host = document.createElement("div")
  host.setAttribute(GATE_LAYER_ATTR, "")
  field = document.createElement("input")
  host.appendChild(field)
  document.body.appendChild(host)
  outside = document.createElement("div")
  document.body.appendChild(outside)
})

afterEach(() => {
  setGateUp(false)
  host.remove()
  outside.remove()
})

function dispatch(target: Element, type: string, init: EventInit = { bubbles: true }) {
  const ev = new Event(type, { cancelable: true, ...init })
  target.dispatchEvent(ev)
  return ev
}

describe("isInGateLayer", () => {
  it("knows the gate's own elements and nothing else", () => {
    expect(isInGateLayer(field)).toBe(true)
    expect(isInGateLayer(outside)).toBe(false)
    expect(isInGateLayer(null)).toBe(false)
  })
})

describe("the capture shield", () => {
  const SHIELDED = [
    "pointerdown",
    "pointermove",
    "pointerup",
    "mousedown",
    "mouseup",
    "dblclick",
    "touchstart",
    "wheel",
    "compositionstart",
    "compositionend",
    "dragover",
    "drop",
  ]

  it.each(SHIELDED)("keeps %s on the gate from the app's document capture listeners", (type) => {
    setGateUp(true)
    const spy = vi.fn()
    document.addEventListener(type, spy, true)
    try {
      dispatch(field, type)
      expect(spy).not.toHaveBeenCalled()
    } finally {
      document.removeEventListener(type, spy, true)
    }
  })

  it("lets the same events through when the gate is down", () => {
    const spy = vi.fn()
    document.addEventListener("pointerdown", spy, true)
    try {
      dispatch(field, "pointerdown")
      expect(spy).toHaveBeenCalledTimes(1)
    } finally {
      document.removeEventListener("pointerdown", spy, true)
    }
  })

  it("leaves events aimed at the app alone", () => {
    setGateUp(true)
    const spy = vi.fn()
    document.addEventListener("pointerdown", spy, true)
    try {
      dispatch(outside, "pointerdown")
      expect(spy).toHaveBeenCalledTimes(1)
    } finally {
      document.removeEventListener("pointerdown", spy, true)
    }
  })

  it("lets a click reach the gate's own button", () => {
    setGateUp(true)
    const button = document.createElement("button")
    host.appendChild(button)
    const onClick = vi.fn()
    button.addEventListener("click", onClick)
    button.click()
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it("stops a file dropped on the gate from navigating the page away", () => {
    setGateUp(true)
    expect(dispatch(field, "dragover").defaultPrevented).toBe(true)
    expect(dispatch(field, "drop").defaultPrevented).toBe(true)
  })
})
