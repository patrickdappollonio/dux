// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

import { useSidebarUnfold } from "./use-sidebar-unfold"

const REDUCED = "(prefers-reduced-motion: reduce)"

// jsdom has no Web Animations API. These stand in for what the hook drives: an
// animation it starts on the panel, and a row's CSS cue animations.
type FakeAnimation = {
  keyframes: Keyframe[]
  playbackRate: number
  reversed: number
  cancelled: boolean
  onfinish: (() => void) | null
  reverse(): void
  cancel(): void
}
let started: FakeAnimation[] = []

function fakeAnimate(keyframes: Keyframe[]): FakeAnimation {
  const animation: FakeAnimation = {
    keyframes,
    playbackRate: 1,
    reversed: 0,
    cancelled: false,
    onfinish: null,
    reverse() {
      this.playbackRate *= -1
      this.reversed += 1
    },
    cancel() {
      this.cancelled = true
    },
  }
  started.push(animation)
  return animation
}

function cue(startTime: number, iterations = Infinity) {
  return {
    animationName: "working-bounce",
    startTime,
    effect: { getTiming: () => ({ iterations }) },
  }
}

let frames: FrameRequestCallback[] = []
function frame() {
  const due = frames
  frames = []
  for (const callback of due) callback(0)
}

let rowCues: ReturnType<typeof cue>[] = []

function Host({ open }: { open: boolean }) {
  const { floating, attachContainer, attachInner } = useSidebarUnfold(true, open)
  return (
    <div ref={attachContainer} data-floating={floating ? "" : undefined}>
      <div ref={attachInner}>
        <div
          data-sidebar-row="agent:s1"
          ref={(el) => {
            if (el) {
              ;(el as unknown as { getAnimations: () => unknown[] }).getAnimations =
                () => rowCues
            }
          }}
        />
      </div>
    </div>
  )
}

let media: MatchMediaStub

beforeEach(() => {
  started = []
  frames = []
  rowCues = []
  media = stubMatchMedia({ [REDUCED]: false })
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback)
    return frames.length
  })
  ;(HTMLElement.prototype as unknown as { animate: unknown }).animate =
    function (keyframes: Keyframe[]) {
      return fakeAnimate(keyframes)
    }
})

afterEach(() => {
  cleanup()
  media.restore()
  vi.unstubAllGlobals()
  delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate
})

// Opens the panel from the rail, whose row's bounce is on the clock 1000, with
// the list's own copy of the row restarted on a clock of its own.
function openFromRail() {
  rowCues = [cue(1000)]
  const view = render(<Host open={false} />)
  view.rerender(<Host open />)
  rowCues = [cue(5000)]
  frame()
  expect(rowCues[0].startTime).toBe(1000)
  return view
}

describe("useSidebarUnfold carrying the working cue's clock", () => {
  it("leaves a cue that has begun settling on the clock it settles against", () => {
    openFromRail()
    // The agent stopped: its bounce now ends with the iteration in flight,
    // sized against this start time.
    rowCues = [cue(5000, 3)]
    frame()
    expect(rowCues[0].startTime).toBe(5000)
  })

  it("leaves a new working cycle on its own clock", () => {
    openFromRail()
    rowCues = []
    frame()
    rowCues = [cue(9000)]
    frame()
    expect(rowCues[0].startTime).toBe(9000)
  })
})

describe("useSidebarUnfold closing", () => {
  it("fades rather than folds when reduced motion was turned on while open", () => {
    const view = render(<Host open={false} />)
    view.rerender(<Host open />)
    const fold = started.find((a) => a.keyframes.some((k) => "clipPath" in k))!
    expect(fold).toBeTruthy()

    act(() => media.set(REDUCED, true))
    view.rerender(<Host open={false} />)
    expect(fold.reversed).toBe(0)
    const fade = started.at(-1)!
    expect(fade.keyframes.every((k) => !("clipPath" in k))).toBe(true)
    expect(fade.playbackRate).toBeLessThan(0)
  })
})
