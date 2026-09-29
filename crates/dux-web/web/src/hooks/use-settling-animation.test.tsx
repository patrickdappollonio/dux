// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  fireAnimationEnd,
  fireAnimationIteration,
  fireAnimationStart,
} from "@/test/animationEvents"

import { useSettlingAnimation } from "./use-settling-animation"

const NAME = "working-pulse"

function Probe({
  active,
  cancel = false,
  timeoutMs,
}: {
  active: boolean
  cancel?: boolean
  timeoutMs?: number
}) {
  const { running, handlers } = useSettlingAnimation(active, NAME, {
    cancel,
    timeoutMs,
  })
  return (
    <div data-testid="probe" className={running ? "animating" : "resting"} {...handlers} />
  )
}

const probe = () => screen.getByTestId("probe")
const animating = () => probe().className === "animating"

// jsdom runs no CSS, so the tests play the browser: `animationstart` says the
// animation really began, `animationiteration` marks each cycle boundary.
const start = (name = NAME) => fireAnimationStart(probe(), name)
const iterate = (name = NAME) =>
  fireAnimationIteration(probe(), name)

describe("useSettlingAnimation", () => {
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it("runs while active", () => {
    render(<Probe active />)
    expect(animating()).toBe(true)
  })

  it("rests when it was never active", () => {
    render(<Probe active={false} />)
    expect(animating()).toBe(false)
  })

  // The whole point: a stop finishes the cycle in flight and comes to rest on
  // its boundary, rather than freezing or snapping mid-cycle.
  it("keeps the animation until the next cycle boundary after work stops", () => {
    const { rerender } = render(<Probe active />)
    start()
    rerender(<Probe active={false} />)
    expect(animating()).toBe(true)

    // Another animation's boundary on the same element is not this one's.
    iterate("working-bounce")
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

  // A boundary while still working is just another cycle.
  it("ignores cycle boundaries while still active", () => {
    render(<Probe active />)
    start()
    iterate()
    iterate()
    expect(animating()).toBe(true)
  })

  // Work coming back before the cycle finishes keeps the SAME animation
  // running: dropping and re-adding the class would restart it from frame zero,
  // which is a jump.
  it("keeps running, with no restart, when work resumes mid-settle", () => {
    const { rerender } = render(<Probe active />)
    start()
    rerender(<Probe active={false} />)
    rerender(<Probe active />)
    expect(animating()).toBe(true)
    // The boundary that would have ended the settle now lands on an active run.
    iterate()
    expect(animating()).toBe(true)

    // And a later stop settles on the next boundary as usual.
    rerender(<Probe active={false} />)
    expect(animating()).toBe(true)
    iterate()
    expect(animating()).toBe(false)
  })

  // Nothing ever started (reduced motion, an element that is not rendered, a
  // test environment): there is no cycle to finish and no boundary will ever
  // come, so the stop is immediate rather than a hang.
  it("stops at once when the animation never actually started", () => {
    const { rerender } = render(<Probe active />)
    rerender(<Probe active={false} />)
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
    // And a boundary arriving late does not revive it.
    iterate()
    expect(animating()).toBe(false)
  })

  // The animation can be cancelled out from under the settle (the element was
  // hidden), and a cancelled animation fires no boundary; a ceiling ends the
  // settle rather than leaving it animating forever.
  it("gives up after the timeout when no boundary arrives", () => {
    vi.useFakeTimers()
    const { rerender } = render(<Probe active timeoutMs={1000} />)
    start()
    rerender(<Probe active={false} timeoutMs={1000} />)
    act(() => vi.advanceTimersByTime(999))
    expect(animating()).toBe(true)
    act(() => vi.advanceTimersByTime(1))
    expect(animating()).toBe(false)
  })

  it("is safe to unmount mid-settle", () => {
    vi.useFakeTimers()
    const errors = vi.spyOn(console, "error").mockImplementation(() => {})
    const { rerender, unmount } = render(<Probe active timeoutMs={1000} />)
    start()
    rerender(<Probe active={false} timeoutMs={1000} />)
    unmount()
    expect(vi.getTimerCount()).toBe(0)
    act(() => vi.advanceTimersByTime(5000))
    expect(errors).not.toHaveBeenCalled()
    errors.mockRestore()
  })

  // A second run after a settled one starts its own bookkeeping: a stop before
  // the new run has actually started is immediate, not a wait on a boundary.
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
