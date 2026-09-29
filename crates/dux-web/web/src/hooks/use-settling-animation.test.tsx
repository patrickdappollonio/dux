// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  fireAnimationCancel,
  fireAnimationEnd,
  fireAnimationIteration,
  fireAnimationStart,
} from "@/test/animationEvents"

import { useSettlingAnimation } from "./use-settling-animation"

const NAME = "working-bounce"

function Probe({ active, cancel = false }: { active: boolean; cancel?: boolean }) {
  const { running, attach, handlers } = useSettlingAnimation<HTMLDivElement>(active, NAME, {
    cancel,
  })
  return (
    <div
      ref={attach}
      data-testid="probe"
      className={running ? "animating" : "resting"}
      {...handlers}
    />
  )
}

const probe = () => screen.getByTestId("probe")
const animating = () => probe().className === "animating"

// jsdom runs no CSS, so the tests play the browser: `animationstart` says the
// animation really began, `animationiteration` marks each cycle boundary.
const start = (name = NAME) => fireAnimationStart(probe(), name)
const iterate = (name = NAME) => fireAnimationIteration(probe(), name)

// A stand-in for the browser's CSSAnimation, which jsdom does not implement:
// just the parts the hook drives (its timing and its finished promise).
function fakeAnimation(animationName: string, currentIteration: number) {
  let resolve!: () => void
  let reject!: (reason: unknown) => void
  const finished = new Promise<void>((res, rej) => {
    resolve = res
    reject = rej
  })
  const updateTiming = vi.fn()
  return {
    animation: {
      animationName,
      finished,
      effect: { getComputedTiming: () => ({ currentIteration }), updateTiming },
    },
    updateTiming,
    finish: async () => {
      await act(async () => resolve())
    },
    cancel: async () => {
      await act(async () => reject(new DOMException("cancelled", "AbortError")))
    },
  }
}

function withAnimations(el: Element, animations: unknown[]) {
  Object.defineProperty(el, "getAnimations", {
    configurable: true,
    value: () => animations,
  })
}

function setHidden(hidden: boolean) {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden })
}

describe("useSettlingAnimation", () => {
  afterEach(() => {
    cleanup()
    setHidden(false)
  })

  it("runs while active", () => {
    render(<Probe active />)
    expect(animating()).toBe(true)
  })

  it("rests when it was never active", () => {
    render(<Probe active={false} />)
    expect(animating()).toBe(false)
  })

  // The exact stop: the running animation is told to end at the end of the
  // iteration it is in, so it finishes ON its resting keyframe, and the class
  // leaves only once the browser says it has finished.
  describe("with the Web Animations API", () => {
    it("ends the running animation at the end of its current iteration", async () => {
      const bounce = fakeAnimation(NAME, 7)
      const pulse = fakeAnimation("working-pulse", 1)
      const { rerender } = render(<Probe active />)
      withAnimations(probe(), [pulse.animation, bounce.animation])
      start()

      rerender(<Probe active={false} />)
      expect(bounce.updateTiming).toHaveBeenCalledWith({ iterations: 8 })
      // Only the named animation is shortened.
      expect(pulse.updateTiming).not.toHaveBeenCalled()
      expect(animating()).toBe(true)

      // A boundary event is not the signal on this path: the browser's own
      // finish is, because an event handled a task later lands a frame late.
      iterate()
      expect(animating()).toBe(true)

      await bounce.finish()
      expect(animating()).toBe(false)
    })

    it("restores infinite iterations, with no restart, when work resumes", async () => {
      const bounce = fakeAnimation(NAME, 2)
      const { rerender } = render(<Probe active />)
      withAnimations(probe(), [bounce.animation])
      start()

      rerender(<Probe active={false} />)
      expect(bounce.updateTiming).toHaveBeenLastCalledWith({ iterations: 3 })
      rerender(<Probe active />)
      expect(bounce.updateTiming).toHaveBeenLastCalledWith({ iterations: Infinity })
      expect(animating()).toBe(true)

      // The old finish arriving late must not stop the resumed run.
      await bounce.finish()
      expect(animating()).toBe(true)
    })

    it("stops when the animation is cancelled mid-settle", async () => {
      const bounce = fakeAnimation(NAME, 0)
      const { rerender } = render(<Probe active />)
      withAnimations(probe(), [bounce.animation])
      start()
      rerender(<Probe active={false} />)
      await bounce.cancel()
      expect(animating()).toBe(false)
    })

    it("stops at once when the element has no such animation", () => {
      const { rerender } = render(<Probe active />)
      withAnimations(probe(), [])
      start()
      rerender(<Probe active={false} />)
      expect(animating()).toBe(false)
    })
  })

  // Where the browser has no `getAnimations`, the next cycle boundary event is
  // the fallback signal.
  describe("without the Web Animations API", () => {
    it("keeps the animation until the next cycle boundary after work stops", () => {
      const { rerender } = render(<Probe active />)
      start()
      rerender(<Probe active={false} />)
      expect(animating()).toBe(true)

      // Another animation's boundary on the same element is not this one's.
      iterate("working-pulse")
      expect(animating()).toBe(true)

      iterate()
      expect(animating()).toBe(false)
    })

    it("settles on animationend too", () => {
      const { rerender } = render(<Probe active />)
      start()
      rerender(<Probe active={false} />)
      fireAnimationEnd(probe(), NAME)
      expect(animating()).toBe(false)
    })

    it("ignores cycle boundaries while still active", () => {
      render(<Probe active />)
      start()
      iterate()
      iterate()
      expect(animating()).toBe(true)
    })

    it("keeps running, with no restart, when work resumes mid-settle", () => {
      const { rerender } = render(<Probe active />)
      start()
      rerender(<Probe active={false} />)
      rerender(<Probe active />)
      expect(animating()).toBe(true)
      iterate()
      expect(animating()).toBe(true)

      rerender(<Probe active={false} />)
      expect(animating()).toBe(true)
      iterate()
      expect(animating()).toBe(false)
    })
  })

  // Nothing ever started (reduced motion, an element that is not rendered, a
  // test environment): there is no cycle to finish and no boundary will ever
  // come, so the stop is immediate rather than a hang.
  it("stops at once when the animation never actually started", () => {
    const { rerender } = render(<Probe active />)
    rerender(<Probe active={false} />)
    expect(animating()).toBe(false)
  })

  // The browser cancels a CSS animation whose element stops rendering, or when
  // reduced motion is switched on mid-run; no boundary follows a cancel.
  it("stops when the browser cancels the animation mid-settle", () => {
    const { rerender } = render(<Probe active />)
    start()
    rerender(<Probe active={false} />)
    expect(animating()).toBe(true)
    fireAnimationCancel(probe(), NAME)
    expect(animating()).toBe(false)
  })

  it("forgets a run the browser cancelled while it was active", () => {
    const { rerender } = render(<Probe active />)
    start()
    fireAnimationCancel(probe(), NAME)
    rerender(<Probe active={false} />)
    expect(animating()).toBe(false)
  })

  // Nothing is painted in a hidden tab, so there is nothing to finish.
  it("stops at once in a hidden tab", () => {
    const { rerender } = render(<Probe active />)
    start()
    setHidden(true)
    rerender(<Probe active={false} />)
    expect(animating()).toBe(false)
  })

  it("stops when the tab is hidden mid-settle", () => {
    const { rerender } = render(<Probe active />)
    start()
    rerender(<Probe active={false} />)
    expect(animating()).toBe(true)
    setHidden(true)
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"))
    })
    expect(animating()).toBe(false)
  })

  // A higher state taking over is not a stop: it takes the element at once.
  it("drops the animation immediately when cancelled mid-settle", () => {
    const { rerender } = render(<Probe active />)
    start()
    rerender(<Probe active={false} />)
    expect(animating()).toBe(true)
    rerender(<Probe active={false} cancel />)
    expect(animating()).toBe(false)
    iterate()
    expect(animating()).toBe(false)
  })

  it("is safe to unmount mid-settle", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {})
    const bounce = fakeAnimation(NAME, 0)
    const { rerender, unmount } = render(<Probe active />)
    withAnimations(probe(), [bounce.animation])
    start()
    rerender(<Probe active={false} />)
    unmount()
    await bounce.finish()
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"))
    })
    expect(errors).not.toHaveBeenCalled()
    errors.mockRestore()
  })

  it("forgets a finished run", () => {
    const { rerender } = render(<Probe active />)
    start()
    rerender(<Probe active={false} />)
    iterate()
    expect(animating()).toBe(false)
    rerender(<Probe active />)
    rerender(<Probe active={false} />)
    expect(animating()).toBe(false)
  })
})
