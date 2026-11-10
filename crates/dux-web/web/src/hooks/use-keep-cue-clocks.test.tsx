// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { useKeepCueClocks } from "./use-keep-cue-clocks"

// jsdom has no Web Animations API, so each pill answers `getAnimations` with
// cue objects the test owns: what the browser does to a moved element (ends its
// animations and starts them over on a new clock) is the test's own assignment.
function cue(name: string, startTime: number, iterations = Infinity) {
  return {
    animationName: name,
    startTime,
    effect: { getTiming: () => ({ iterations }) },
  }
}
type Cue = ReturnType<typeof cue>
const cues = new Map<string, Cue[]>()

function Strip({ order }: { order: string[] }) {
  const attach = useKeepCueClocks(order.join(","))
  return (
    <div ref={attach}>
      {order.map((id) => (
        <div
          key={id}
          data-cue-key={id}
          ref={(el) => {
            if (el) {
              ;(el as unknown as { getAnimations: () => Cue[] }).getAnimations = () =>
                cues.get(id) ?? []
            }
          }}
        />
      ))}
    </div>
  )
}

let frames: FrameRequestCallback[] = []
function frame() {
  const due = frames
  frames = []
  for (const callback of due) callback(0)
}

function working(id: string, start: number) {
  cues.set(id, [cue("working-bounce", start), cue("working-pulse", start)])
}

beforeEach(() => {
  cues.clear()
  frames = []
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback)
    return frames.length
  })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("useKeepCueClocks", () => {
  it("hands a moved working pill back the clock its cue was on", () => {
    working("a", 1000)
    working("b", 1000)
    const view = render(<Strip order={["a", "b"]} />)

    view.rerender(<Strip order={["b", "a"]} />)
    // The browser restarted the moved pill's cue; its neighbour ran on.
    working("a", 5000)
    frame()

    expect(cues.get("a")?.map((c) => c.startTime)).toEqual([1000, 1000])
    expect(cues.get("b")?.map((c) => c.startTime)).toEqual([1000, 1000])
  })

  it("keeps correcting a pill the browser replaces again a frame later", () => {
    working("a", 1000)
    const view = render(<Strip order={["a", "b"]} />)
    view.rerender(<Strip order={["b", "a"]} />)
    working("a", 5000)
    frame()
    working("a", 7000)
    frame()
    expect(cues.get("a")?.[0].startTime).toBe(1000)
  })

  it("leaves a cue that has begun settling on the clock it settles against", () => {
    working("a", 1000)
    const view = render(<Strip order={["a", "b"]} />)
    view.rerender(<Strip order={["b", "a"]} />)
    cues.set("a", [cue("working-bounce", 5000, 3)])
    frame()
    expect(cues.get("a")?.[0].startTime).toBe(5000)
  })

  it("touches nothing when the order did not change", () => {
    working("a", 1000)
    const view = render(<Strip order={["a", "b"]} />)
    working("a", 5000)
    view.rerender(<Strip order={["a", "b"]} />)
    frame()
    expect(cues.get("a")?.[0].startTime).toBe(5000)
  })

  it("leaves a pill that was not working before the move on its own clock", () => {
    working("a", 1000)
    const view = render(<Strip order={["a", "b"]} />)
    view.rerender(<Strip order={["b", "a"]} />)
    working("b", 9000)
    frame()
    expect(cues.get("b")?.[0].startTime).toBe(9000)
  })
})
