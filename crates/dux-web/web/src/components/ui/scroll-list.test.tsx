// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { resolve } from "node:path"
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

describe("ScrollList enclosing, where the box is and who shares it", () => {
  it("keeps a shared box's shadow until the last list on it goes", () => {
    function Box({ first, second }: { first: boolean; second: boolean }) {
      return (
        <div data-slot="modal-body" data-testid="body">
          {first ? <ScrollList scroll="enclosing">one</ScrollList> : null}
          {second ? <ScrollList scroll="enclosing">two</ScrollList> : null}
        </div>
      )
    }
    const { rerender } = render(<Box first second />)
    const body = screen.getByTestId("body")
    scrollTo(body, 300)
    rerender(<Box first={false} second />)
    expect(edges(body)).toEqual({ fades: true, above: true, below: true })
    rerender(<Box first={false} second={false} />)
    expect(edges(body)).toEqual({ fades: false, above: false, below: false })
  })

  it("leaves a modal body alone when the list sits in a list box that scrolls", () => {
    render(
      <div data-slot="modal-body" data-testid="body">
        <ScrollList scroll="own" height="fill" scrollbar="native">
          <ScrollList scroll="enclosing">row</ScrollList>
        </ScrollList>
      </div>,
    )
    expect(edges(screen.getByTestId("body")).fades).toBe(false)
  })

  it("shadows any other box that scrolls, in the way", () => {
    render(
      <div data-slot="modal-body" data-testid="body">
        <div style={{ overflowY: "auto" }} data-testid="other">
          <ScrollList scroll="enclosing">row</ScrollList>
        </div>
      </div>,
    )
    const other = screen.getByTestId("other")
    scrollTo(other, 600)
    expect(edges(other)).toEqual({ fades: true, above: true, below: false })
    expect(edges(screen.getByTestId("body")).fades).toBe(false)
  })

  it("says so when nothing around it scrolls", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    render(<ScrollList scroll="enclosing">row</ScrollList>)
    expect(error).toHaveBeenCalledWith(expect.stringContaining("no scrolling ancestor"))
    error.mockRestore()
  })
})

describe("the shadow leaves a scrollbar whole", () => {
  const withScrollbar = (el: HTMLElement) => {
    Object.defineProperty(el, "offsetWidth", { configurable: true, value: 215 })
    Object.defineProperty(el, "clientWidth", { configurable: true, value: 200 })
    scrollTo(el, 0)
  }

  it("tells the stylesheet how wide the scrollbar of the box is, own or enclosing", () => {
    const { container } = render(
      <ScrollList scroll="own" height="fill" scrollbar="native">
        row
      </ScrollList>,
    )
    const own = container.firstElementChild as HTMLElement
    expect(own.style.getPropertyValue("--dux-list-fade-bar")).toBe("0px")
    withScrollbar(own)
    expect(own.style.getPropertyValue("--dux-list-fade-bar")).toBe("15px")
    cleanup()

    render(
      <div data-slot="modal-body" data-testid="body">
        <ScrollList scroll="enclosing">row</ScrollList>
      </div>,
    )
    const body = screen.getByTestId("body")
    withScrollbar(body)
    expect(body.style.getPropertyValue("--dux-list-fade-bar")).toBe("15px")
  })

  it("adds the strip and a focus ring's edges back opaque, and keeps the viewport's own transition", () => {
    const css = readFileSync(resolve(process.cwd(), "src/index.css"), "utf8")
    const rule = (selector: string) => css.slice(css.indexOf(`${selector} {`)).split("}")[0]
    expect(rule("[data-scroll-list-fade]")).toMatch(/mask-size:\s*calc\(100% - var\(--dux-list-fade-bar\)\) 100%/)
    expect(rule("[data-scroll-list-fade]:focus-visible")).toContain("--dux-list-fade-ring: 3px")
    expect(rule('[data-scroll-list-fade][data-slot="scroll-area-viewport"]')).toMatch(/box-shadow/)
  })
})

describe("ScrollList reveal", () => {
  it("drops the scroll padding of a box that must rest on whole rows", () => {
    const { container } = render(
      <ScrollList scroll="own" height="fill" scrollbar="gutter" reveal="flush">
        row
      </ScrollList>,
    )
    expect((container.firstElementChild as HTMLElement).hasAttribute("data-scroll-list-flush")).toBe(true)
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
