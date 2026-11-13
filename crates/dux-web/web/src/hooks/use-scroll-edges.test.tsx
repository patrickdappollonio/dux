// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { hasMoreAbove, hasMoreBelow, useScrollEdges } from "@/hooks/use-scroll-edges"

describe("hasMoreBelow", () => {
  it("is on while the list can scroll further down", () => {
    expect(hasMoreBelow({ scrollTop: 0, scrollHeight: 1000, clientHeight: 400 })).toBe(true)
    expect(hasMoreBelow({ scrollTop: 300, scrollHeight: 1000, clientHeight: 400 })).toBe(true)
  })

  it("is off at the bottom, including a sub-pixel short of it", () => {
    expect(hasMoreBelow({ scrollTop: 600, scrollHeight: 1000, clientHeight: 400 })).toBe(false)
    expect(hasMoreBelow({ scrollTop: 599.5, scrollHeight: 1000, clientHeight: 400 })).toBe(false)
  })

  it("is off when the content fits", () => {
    expect(hasMoreBelow({ scrollTop: 0, scrollHeight: 400, clientHeight: 400 })).toBe(false)
    expect(hasMoreBelow({ scrollTop: 0, scrollHeight: 200, clientHeight: 400 })).toBe(false)
  })
})

describe("hasMoreAbove", () => {
  it("is on once the list has scrolled down", () => {
    expect(hasMoreAbove({ scrollTop: 300, scrollHeight: 1000, clientHeight: 400 })).toBe(true)
    expect(hasMoreAbove({ scrollTop: 600, scrollHeight: 1000, clientHeight: 400 })).toBe(true)
  })

  it("is off at the top, including a sub-pixel below it", () => {
    expect(hasMoreAbove({ scrollTop: 0, scrollHeight: 1000, clientHeight: 400 })).toBe(false)
    expect(hasMoreAbove({ scrollTop: 0.5, scrollHeight: 1000, clientHeight: 400 })).toBe(false)
  })
})

// jsdom does no layout, so the test sets the metrics the browser would report.
function setMetrics(el: HTMLElement, m: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
  Object.defineProperty(el, "scrollHeight", { configurable: true, value: m.scrollHeight })
  Object.defineProperty(el, "clientHeight", { configurable: true, value: m.clientHeight })
  el.scrollTop = m.scrollTop
}

function Probe({ rows }: { rows: number }) {
  const { ref, moreAbove, moreBelow } = useScrollEdges<HTMLDivElement>()
  return (
    <div
      ref={ref}
      data-testid="list"
      data-more-above={moreAbove ? "" : undefined}
      data-more-below={moreBelow ? "" : undefined}
    >
      {Array.from({ length: rows }, (_, i) => (
        <div key={i}>row {i}</div>
      ))}
    </div>
  )
}

function scrollTo(list: HTMLElement, scrollTop: number) {
  setMetrics(list, { scrollTop, scrollHeight: 1000, clientHeight: 400 })
  act(() => {
    list.dispatchEvent(new Event("scroll"))
  })
}

describe("useScrollEdges", () => {
  afterEach(cleanup)

  it("follows scrolls and rows arriving, at both edges", async () => {
    const { getByTestId, rerender } = render(<Probe rows={1} />)
    const list = getByTestId("list")
    expect(list.hasAttribute("data-more-above")).toBe(false)
    expect(list.hasAttribute("data-more-below")).toBe(false)

    // Rows arrive and overflow the box: the child-list change re-measures.
    setMetrics(list, { scrollTop: 0, scrollHeight: 1000, clientHeight: 400 })
    rerender(<Probe rows={20} />)
    await act(async () => {})
    expect(list.hasAttribute("data-more-above")).toBe(false)
    expect(list.hasAttribute("data-more-below")).toBe(true)

    // In the middle: both edges fade.
    scrollTo(list, 300)
    expect(list.hasAttribute("data-more-above")).toBe(true)
    expect(list.hasAttribute("data-more-below")).toBe(true)

    // At the bottom: only the top fades.
    scrollTo(list, 600)
    expect(list.hasAttribute("data-more-above")).toBe(true)
    expect(list.hasAttribute("data-more-below")).toBe(false)

    // Back to the top: only the bottom fades.
    scrollTo(list, 0)
    expect(list.hasAttribute("data-more-above")).toBe(false)
    expect(list.hasAttribute("data-more-below")).toBe(true)
  })
})
