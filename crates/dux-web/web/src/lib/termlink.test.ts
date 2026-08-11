// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"

import {
  activateLinkAtPoint,
  linkifierElement,
  terminalTapAction,
} from "./termlink"

/**
 * Stands in for what xterm builds under `Terminal.element`: an outer element
 * carrying the focus grab / selection / mouse-report listeners, and the inner
 * `.xterm-screen` the Linkifier binds to. The probe must reach the second and
 * never the first.
 */
function openTerminal() {
  const root = document.createElement("div")
  root.className = "xterm"
  const screen = document.createElement("div")
  screen.className = "xterm-screen"
  root.appendChild(screen)
  document.body.appendChild(root)
  const outer: string[] = []
  for (const type of ["mousemove", "mousedown", "mouseup", "mouseleave"]) {
    root.addEventListener(type, () => outer.push(type))
  }
  return { root, screen, outer }
}

afterEach(() => {
  document.body.innerHTML = ""
})

describe("linkifierElement", () => {
  it("finds the screen element xterm binds the Linkifier to", () => {
    const { root, screen } = openTerminal()
    expect(linkifierElement(root)).toBe(screen)
  })

  it("answers null before the terminal is open", () => {
    expect(linkifierElement(null)).toBe(null)
    expect(linkifierElement(undefined)).toBe(null)
    expect(linkifierElement(document.createElement("div"))).toBe(null)
  })
})

describe("activateLinkAtPoint", () => {
  it("delivers the mousemove/mousedown/mouseup sequence the Linkifier needs", () => {
    const { screen } = openTerminal()
    const seen: string[] = []
    for (const type of ["mousemove", "mousedown", "mouseup", "mouseleave"]) {
      screen.addEventListener(type, () => seen.push(type))
    }
    activateLinkAtPoint(screen, 10, 20, () => 0)
    expect(seen).toEqual([
      // The first move primes a different cell; see the priming comment.
      "mousemove",
      "mousemove",
      "mousedown",
      "mouseup",
      "mouseleave",
    ])
  })

  it("primes from a different point before hovering the tapped one", () => {
    const { screen } = openTerminal()
    screen.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 400, bottom: 200, width: 400, height: 200 }) as DOMRect
    const moves: MouseEvent[] = []
    screen.addEventListener("mousemove", (e) => moves.push(e as MouseEvent))
    activateLinkAtPoint(screen, 20, 47, () => 0)
    expect(moves).toHaveLength(2)
    // Same row, far side of the element: a different CELL, still inside it.
    expect(moves[0].clientX).toBe(399)
    expect(moves[0].clientY).toBe(47)
    expect(moves[1].clientX).toBe(20)
    // ...and from the other half it primes from the near edge instead.
    moves.length = 0
    activateLinkAtPoint(screen, 380, 47, () => 0)
    expect(moves[0].clientX).toBe(1)
  })

  it("carries the tapped point and a primary single click", () => {
    const { screen } = openTerminal()
    const events: MouseEvent[] = []
    for (const type of ["mousedown", "mouseup"]) {
      screen.addEventListener(type, (e) => events.push(e as MouseEvent))
    }
    activateLinkAtPoint(screen, 133, 47, () => 0)
    for (const e of events) {
      expect(e.clientX).toBe(133)
      expect(e.clientY).toBe(47)
      // linkActivateAction refuses a non-primary button and the tail of a
      // multi-click gesture, so the replay has to claim neither.
      expect(e.button).toBe(0)
      expect(e.detail).toBe(1)
    }
  })

  it("does not bubble, so xterm's focus grab, selection and mouse reports never see it", () => {
    const { screen, outer } = openTerminal()
    activateLinkAtPoint(screen, 5, 5, () => 0)
    expect(outer).toEqual([])
  })

  it("reports a hit when the activation counter moved during the mouseup", () => {
    const { screen } = openTerminal()
    let activations = 0
    screen.addEventListener("mouseup", () => {
      activations++
    })
    expect(activateLinkAtPoint(screen, 1, 1, () => activations)).toBe(true)
  })

  it("reports an ordinary tap when nothing activated", () => {
    const { screen } = openTerminal()
    expect(activateLinkAtPoint(screen, 1, 1, () => 7)).toBe(false)
  })

  it("does not count an activation that the trailing mouseleave produced", () => {
    // The reset event must not be able to fake a hit.
    const { screen } = openTerminal()
    let activations = 0
    screen.addEventListener("mouseleave", () => {
      activations++
    })
    expect(activateLinkAtPoint(screen, 1, 1, () => activations)).toBe(false)
  })

  it("is a no-op before the terminal is open", () => {
    expect(activateLinkAtPoint(null, 1, 1, () => 0)).toBe(false)
  })
})

describe("terminalTapAction", () => {
  it("focuses the compose box on an ordinary tap", () => {
    expect(
      terminalTapAction({ linkActivated: false, mouseTracking: false }),
    ).toEqual({ forwardClick: false, focusCompose: true })
  })

  it("leaves focus alone when the tap opened a link", () => {
    expect(
      terminalTapAction({ linkActivated: true, mouseTracking: false }),
    ).toEqual({ forwardClick: false, focusCompose: false })
  })

  it("still forwards the SGR click to a mouse-tracking app on an ordinary tap", () => {
    expect(
      terminalTapAction({ linkActivated: false, mouseTracking: true }),
    ).toEqual({ forwardClick: true, focusCompose: true })
  })

  it("forwards the SGR click on a link tap too, as a desktop click does both", () => {
    expect(
      terminalTapAction({ linkActivated: true, mouseTracking: true }),
    ).toEqual({ forwardClick: true, focusCompose: false })
  })
})
