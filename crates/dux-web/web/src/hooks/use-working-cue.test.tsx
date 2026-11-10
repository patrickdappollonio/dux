// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { Bot } from "lucide-react"
import { useRef } from "react"

import { WorkingGlyph } from "@/components/WorkingGlyph"
import { expectLookEasesIn, fireAnimationStart, stubGlyphEase } from "@/test/animationEvents"

import {
  useWorkingCue,
  useWorkingPulseAnchor,
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
  const fade = { cancel: vi.fn(), finished: new Promise<void>(() => {}) }
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

// The state word pulses in step with the glyph, so when work resumes while the
// last bounce is still in flight its restarted pulse is anchored to the same
// bounce the glyph's pulse is.
function AnchorProbe({ working }: { working: boolean }) {
  const frameRef = useRef<HTMLSpanElement>(null)
  const wordRef = useRef<HTMLSpanElement>(null)
  useWorkingPulseAnchor(working, frameRef, wordRef)
  return (
    <>
      <span ref={frameRef} data-testid="frame" />
      <span ref={wordRef} data-testid="word" />
    </>
  )
}

describe("useWorkingPulseAnchor", () => {
  afterEach(() => cleanup())

  it("anchors another element's pulse to the bounce's start on resume", () => {
    const bounce = fakeAnimation("working-bounce", 3, 1000)
    const word = fakeAnimation("working-pulse", 0, 2345)
    const { rerender } = render(<AnchorProbe working={false} />)
    withAnimations(screen.getByTestId("frame"), () => [bounce.animation])
    withAnimations(screen.getByTestId("word"), () => [word.animation])
    rerender(<AnchorProbe working />)
    expect(word.animation.startTime).toBe(1000)
    expect(bounce.clockWrites).toEqual([])
  })

  it("leaves a pulse already in step alone", () => {
    const bounce = fakeAnimation("working-bounce", 0, 50)
    const word = fakeAnimation("working-pulse", 0, 50)
    const { rerender } = render(<AnchorProbe working={false} />)
    withAnimations(screen.getByTestId("frame"), () => [bounce.animation])
    withAnimations(screen.getByTestId("word"), () => [word.animation])
    rerender(<AnchorProbe working />)
    expect(word.clockWrites).toEqual([])
  })
})

// A look held for the landing must never be held forever: a stop that comes
// before the bounce ever reported starting still holds it for the ease, and a
// hidden tab cancelling that ease releases it.
describe("WorkingGlyph's held look", () => {
  afterEach(() => {
    cleanup()
    setHidden(false)
  })

  it("holds the look through the ease and releases it when a hidden tab cancels it", async () => {
    const { container, rerender } = render(<WorkingGlyph icon={Bot} working look="at-rest" />)
    const frame = () => container.querySelector("[data-slot='working-glyph']")!
    stubGlyphEase(container.querySelector("svg")!)

    rerender(<WorkingGlyph icon={Bot} working={false} look="needs-you" />)
    expect(frame().className).not.toContain("needs-you")

    setHidden(true)
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"))
    })
    expect(frame().className).toContain("needs-you")
  })
})

// A glyph that reports a part rather than names it (the phone's agent/terminals
// switch, the computer's layout pill dot) pulses and lands like any working
// glyph but never bounces, and carries its cue's clock by key so a copy of it
// remounted elsewhere carries on in step.
describe("a working glyph without the bounce", () => {
  const ATTENTION =
    "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
  afterEach(() => {
    cleanup()
    setHidden(false)
    vi.unstubAllGlobals()
  })

  it("pulses, and never puts the bounce on its frame", () => {
    const { container, rerender } = render(<WorkingGlyph icon={Bot} working bounce={false} />)
    const frame = container.querySelector("[data-slot='working-glyph']")!
    expect(container.querySelector("svg")!.getAttribute("class")).toContain(WORKING_PULSE_ANIMATION)
    expect(frame.className).not.toContain("bounce")
    rerender(<WorkingGlyph icon={Bot} working={false} bounce={false} />)
    expect(frame.className).not.toContain("bounce")
  })

  it("lets the pulse land before the attention look eases in, then blinks", async () => {
    const { container, rerender } = render(
      <WorkingGlyph icon={Bot} working bounce={false} look="text-dux-working" />,
    )
    const frame = () => container.querySelector("[data-slot='working-glyph']")!
    const ease = stubGlyphEase(container.querySelector("svg")!)
    rerender(<WorkingGlyph icon={Bot} working={false} bounce={false} look={ATTENTION} />)
    // The pulse yields at once and the glyph eases back from where it was.
    expect(container.querySelector("svg")!.getAttribute("class")).not.toContain("animate-working-pulse")
    expect(ease.animate).toHaveBeenCalledTimes(1)
    expect(frame().className).not.toContain("text-cyan-100")
    await ease.eased()
    expect(frame().className).toContain("text-cyan-100")
    expectLookEasesIn(frame().className, "the attention look after the landing")
  })

  it("snaps to the new look under reduced motion", () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn((query: string) => ({
        matches: query.includes("prefers-reduced-motion"),
        media: query,
        addEventListener: () => {},
        removeEventListener: () => {},
      })),
    )
    const { container, rerender } = render(
      <WorkingGlyph icon={Bot} working bounce={false} look="text-dux-working" />,
    )
    stubGlyphEase(container.querySelector("svg")!)
    rerender(<WorkingGlyph icon={Bot} working={false} bounce={false} look={ATTENTION} />)
    const frame = container.querySelector("[data-slot='working-glyph']")!
    expect(frame.className).toContain("text-cyan-100")
    expect(frame.className).not.toContain("duration-200")
  })

  // jsdom has no Web Animations: each element reports the cue animations its
  // classes would run, with a clock of its own that the glyph may move.
  function fakeCueClocks(initial: (el: Element, name: string) => number) {
    const clocks = new Map<string, { startTime: number | null }>()
    const ids = new WeakMap<Element, number>()
    let next = 0
    const original = Element.prototype.getAnimations
    Element.prototype.getAnimations = function (this: Element) {
      const cls = this.getAttribute("class") ?? ""
      if (!ids.has(this)) ids.set(this, next++)
      const names = ["working-pulse", "attention-pulse"].filter((n) => cls.includes(`animate-${n}`))
      return names.map((name) => {
        const key = `${ids.get(this)}|${name}`
        if (!clocks.has(key)) clocks.set(key, { startTime: initial(this, name) })
        const delay = cls.includes("[animation-delay:200ms]") ? 200 : 0
        return Object.assign(clocks.get(key)!, {
          animationName: name,
          effect: { getComputedTiming: () => ({ duration: 1600, delay }) },
        })
      }) as unknown as Animation[]
    }
    return {
      start: (el: Element, name: string) => clocks.get(`${ids.get(el)}|${name}`)?.startTime,
      restore: () => {
        Element.prototype.getAnimations = original
      },
    }
  }

  it("carries a running cue's clock to a remounted glyph, the look's delay accounted for", async () => {
    let t = 1000
    const clocks = fakeCueClocks(() => (t += 1111))
    try {
      const flap = render(<WorkingGlyph icon={Bot} working bounce={false} cueKey="k1" />)
      const flapPulse = clocks.start(flap.container.querySelector("svg")!, "working-pulse")
      const pill = render(<WorkingGlyph icon={Bot} working bounce={false} cueKey="k1" />)
      expect(clocks.start(pill.container.querySelector("svg")!, "working-pulse")).toBe(flapPulse)
      flap.unmount()
      pill.unmount()

      // A blink that landed with its delay, and the same blink mounted afresh
      // without one, show the same frame.
      const ATTENTION = "text-cyan-100 motion-safe:animate-attention-pulse"
      const landed = render(<WorkingGlyph icon={Bot} working bounce={false} look="x" cueKey="k2" />)
      const ease = stubGlyphEase(landed.container.querySelector("svg")!)
      landed.rerender(<WorkingGlyph icon={Bot} working={false} bounce={false} look={ATTENTION} cueKey="k2" />)
      await ease.eased()
      const landedFrame = landed.container.querySelector("[data-slot='working-glyph']")!
      expect(landedFrame.className).toContain("[animation-delay:200ms]")
      const landedStart = clocks.start(landedFrame, "attention-pulse")!
      const again = render(<WorkingGlyph icon={Bot} working={false} bounce={false} look={ATTENTION} cueKey="k2" />)
      const againFrame = again.container.querySelector("[data-slot='working-glyph']")!
      expect(clocks.start(againFrame, "attention-pulse")).toBe(landedStart + 200)
    } finally {
      clocks.restore()
    }
  })

  // The layout pill's dot is gone while the agent is idle, so each working or
  // needs-you spell mounts a new glyph: it must start that spell afresh, as the
  // list's glyph does, never on the clock of a spell that already ended.
  it.each([
    ["a pulse", { working: true, look: undefined }, "working-pulse", "svg"],
    [
      "a needs-you blink",
      { working: false, look: "text-cyan-100 motion-safe:animate-attention-pulse" },
      "attention-pulse",
      "[data-slot='working-glyph']",
    ],
  ] as const)("starts %s afresh after the glyph was gone between two spells", async (_what, props, name, part) => {
    let t = 1000
    const clocks = fakeCueClocks(() => (t += 1111))
    try {
      const first = render(<WorkingGlyph icon={Bot} bounce={false} cueKey="dot" {...props} />)
      const firstStart = clocks.start(first.container.querySelector(part)!, name)
      first.unmount()
      await act(async () => {})
      const second = render(<WorkingGlyph icon={Bot} bounce={false} cueKey="dot" {...props} />)
      const own = t
      const secondStart = clocks.start(second.container.querySelector(part)!, name)
      expect(secondStart).toBe(own)
      expect(secondStart).not.toBe(firstStart)
    } finally {
      clocks.restore()
    }
  })

  it("starts a new cue at once, as the list does, never waiting for another clock", async () => {
    let t = 1000
    const clocks = fakeCueClocks(() => (t += 1111))
    try {
      // Another instance's clock is on record.
      render(<WorkingGlyph icon={Bot} working bounce={false} cueKey="k3" />)
      const idle = render(<WorkingGlyph icon={Bot} working={false} bounce={false} look="x" cueKey="k3" />)
      idle.rerender(<WorkingGlyph icon={Bot} working bounce={false} look="x" cueKey="k3" />)
      const svg = idle.container.querySelector("svg")!
      const own = t
      expect(clocks.start(svg, "working-pulse")).toBe(own)

      // A hand-over: the blink lands on the list's schedule, the colour easing
      // in over 200ms and the blink 200ms behind it, its clock untouched.
      const ease = stubGlyphEase(svg)
      idle.rerender(
        <WorkingGlyph
          icon={Bot}
          working={false}
          bounce={false}
          look="text-cyan-100 motion-safe:animate-attention-pulse"
          cueKey="k3"
        />,
      )
      await ease.eased()
      const frame = idle.container.querySelector("[data-slot='working-glyph']")!
      expectLookEasesIn(frame.className, "the switch's blink after the landing")
      expect(clocks.start(frame, "attention-pulse")).toBe(t)
    } finally {
      clocks.restore()
    }
  })
})
