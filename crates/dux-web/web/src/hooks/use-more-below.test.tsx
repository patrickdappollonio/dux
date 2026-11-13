// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { hasMoreBelow, useMoreBelow } from "@/hooks/use-more-below"

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

// jsdom does no layout, so the test sets the metrics the browser would report.
function setMetrics(el: HTMLElement, m: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
  Object.defineProperty(el, "scrollHeight", { configurable: true, value: m.scrollHeight })
  Object.defineProperty(el, "clientHeight", { configurable: true, value: m.clientHeight })
  el.scrollTop = m.scrollTop
}

function Probe({ rows }: { rows: number }) {
  const { ref, moreBelow } = useMoreBelow<HTMLDivElement>()
  return (
    <div ref={ref} data-testid="list" data-more-below={moreBelow ? "" : undefined}>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i}>row {i}</div>
      ))}
    </div>
  )
}

describe("useMoreBelow", () => {
  afterEach(cleanup)

  it("follows scrolls and rows arriving", async () => {
    const { getByTestId, rerender } = render(<Probe rows={1} />)
    const list = getByTestId("list")
    expect(list.hasAttribute("data-more-below")).toBe(false)

    // Rows arrive and overflow the box: the child-list change re-measures.
    setMetrics(list, { scrollTop: 0, scrollHeight: 1000, clientHeight: 400 })
    rerender(<Probe rows={20} />)
    await act(async () => {})
    expect(list.hasAttribute("data-more-below")).toBe(true)

    // Scrolled to the bottom: the fade goes.
    setMetrics(list, { scrollTop: 600, scrollHeight: 1000, clientHeight: 400 })
    act(() => {
      list.dispatchEvent(new Event("scroll"))
    })
    expect(list.hasAttribute("data-more-below")).toBe(false)

    // Back up: it returns.
    setMetrics(list, { scrollTop: 100, scrollHeight: 1000, clientHeight: 400 })
    act(() => {
      list.dispatchEvent(new Event("scroll"))
    })
    expect(list.hasAttribute("data-more-below")).toBe(true)
  })
})
