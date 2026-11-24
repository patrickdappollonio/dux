// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { ScrollList, ScrollListRows, scrollListMoveTarget } from "@/components/ui/scroll-list"

// jsdom does no layout, so a test sets the numbers a browser would report.
function setMetrics(el: HTMLElement, m: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
  Object.defineProperty(el, "scrollHeight", { configurable: true, value: m.scrollHeight })
  Object.defineProperty(el, "clientHeight", { configurable: true, value: m.clientHeight })
  el.scrollTop = m.scrollTop
}

function scrollTo(el: HTMLElement, scrollTop: number) {
  setMetrics(el, { scrollTop, scrollHeight: 1000, clientHeight: 400 })
  act(() => {
    el.dispatchEvent(new Event("scroll"))
  })
}

const edges = (el: HTMLElement) => ({
  fades: el.hasAttribute("data-scroll-list-fade"),
  above: el.hasAttribute("data-more-above"),
  below: el.hasAttribute("data-more-below"),
})

afterEach(cleanup)

describe("ScrollList with its own box", () => {
  it("shows the end shadow while more is below, the start one once scrolled, none when it fits", () => {
    const { container } = render(
      <ScrollList scroll="own" height="fill" scrollbar="hidden">
        <div>row</div>
      </ScrollList>,
    )
    const box = container.firstElementChild as HTMLElement
    expect(edges(box)).toEqual({ fades: true, above: false, below: false })

    scrollTo(box, 0)
    expect(edges(box)).toEqual({ fades: true, above: false, below: true })
    scrollTo(box, 300)
    expect(edges(box)).toEqual({ fades: true, above: true, below: true })
    scrollTo(box, 600)
    expect(edges(box)).toEqual({ fades: true, above: true, below: false })

    setMetrics(box, { scrollTop: 0, scrollHeight: 400, clientHeight: 400 })
    act(() => {
      box.dispatchEvent(new Event("scroll"))
    })
    expect(edges(box)).toEqual({ fades: true, above: false, below: false })
  })

  it("hands the scrolling box to the caller", () => {
    const scrollerRef = vi.fn()
    const { container } = render(
      <ScrollList scroll="own" height="fill" scrollbar="hidden" scrollerRef={scrollerRef}>
        <div>row</div>
      </ScrollList>,
    )
    expect(scrollerRef).toHaveBeenCalledWith(container.firstElementChild)
  })
})

describe("ScrollList inside a scrolling box it does not own", () => {
  it("puts the shadows on a modal's body, and takes them away when it leaves", () => {
    function Body({ list }: { list: boolean }) {
      return (
        <div data-slot="modal-body" data-testid="body">
          {list ? (
            <ScrollList scroll="enclosing">
              <div>row</div>
            </ScrollList>
          ) : (
            <p>an editor</p>
          )}
        </div>
      )
    }
    const { rerender } = render(<Body list />)
    const body = screen.getByTestId("body")
    scrollTo(body, 300)
    expect(edges(body)).toEqual({ fades: true, above: true, below: true })

    rerender(<Body list={false} />)
    expect(edges(body)).toEqual({ fades: false, above: false, below: false })
  })

  it("puts them on the scroll area a modal body scrolls in, or the one the list sits in", () => {
    render(
      <div data-slot="scroll-area-viewport" data-testid="modal-viewport">
        <div data-slot="modal-body">
          <ScrollList scroll="enclosing">
            <div>row</div>
          </ScrollList>
        </div>
      </div>,
    )
    const viewport = screen.getByTestId("modal-viewport")
    scrollTo(viewport, 600)
    expect(edges(viewport)).toEqual({ fades: true, above: true, below: false })
    cleanup()

    render(
      <div data-slot="modal-body" data-testid="body">
        <div data-slot="scroll-area-viewport" data-testid="own-viewport">
          <ScrollList scroll="enclosing">
            <div>row</div>
          </ScrollList>
        </div>
      </div>,
    )
    const own = screen.getByTestId("own-viewport")
    scrollTo(own, 0)
    expect(edges(own)).toEqual({ fades: true, above: false, below: true })
    expect(edges(screen.getByTestId("body")).fades).toBe(false)
  })
})

describe("ScrollListRows", () => {
  it("renders its children as they are", () => {
    render(
      <ScrollListRows gap="normal" semantics="list">
        <li>
          <button type="button" data-x="kept">
            Alpha
          </button>
        </li>
      </ScrollListRows>,
    )
    const button = screen.getByRole("button", { name: "Alpha" })
    expect(button.getAttribute("data-x")).toBe("kept")
    expect(screen.getByRole("list").contains(button)).toBe(true)
  })

  it("moves through a listbox with the arrows, Home and End", () => {
    const onMove = vi.fn()
    render(
      <ScrollListRows gap="tight" semantics="listbox" label="Things" movement={{ count: 3, at: 1, onMove }}>
        <li role="option" aria-selected tabIndex={0}>
          one
        </li>
      </ScrollListRows>,
    )
    const listbox = screen.getByRole("listbox", { name: "Things" })
    for (const key of ["ArrowDown", "ArrowUp", "Home", "End"]) {
      expect(fireEvent.keyDown(screen.getByRole("option"), { key })).toBe(false)
    }
    expect(onMove.mock.calls.map(([i]) => i)).toEqual([2, 0, 0, 2])
    // Any other key is left alone.
    expect(fireEvent.keyDown(listbox, { key: "a" })).toBe(true)
    expect(onMove).toHaveBeenCalledTimes(4)
  })
})

describe("scrollListMoveTarget", () => {
  it("clamps at both ends, and starts from the first row when none is current", () => {
    expect(scrollListMoveTarget("ArrowDown", 2, 3)).toBe(2)
    expect(scrollListMoveTarget("ArrowUp", 0, 3)).toBe(0)
    expect(scrollListMoveTarget("ArrowDown", -1, 3)).toBe(0)
    expect(scrollListMoveTarget("ArrowUp", -1, 3)).toBe(0)
    expect(scrollListMoveTarget("End", -1, 3)).toBe(2)
    expect(scrollListMoveTarget("Tab", 1, 3)).toBeNull()
  })
})
