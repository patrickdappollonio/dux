// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import { TabPill, TabStrip, TabStripButton } from "./TabStrip"

afterEach(cleanup)

function strip(onSelect = vi.fn(), pillProps: Record<string, unknown> = {}) {
  return render(
    <TabStrip label="Things" besidePaneDivider={false} trailing={<TabStripButton aria-label="Add" />}>
      <TabPill
        active
        onSelect={onSelect}
        icon={null}
        label="one"
        trailing={<button type="button">Close one</button>}
        {...pillProps}
      />
      <TabPill active={false} onSelect={vi.fn()} icon={null} label="two" />
    </TabStrip>,
  )
}

describe("TabPill", () => {
  it("selects on Enter or Space on the pill, and leaves a control inside it its own keys", () => {
    const onSelect = vi.fn()
    strip(onSelect)
    const [pill] = screen.getAllByRole("tab")
    // A key that is not cancelled goes on to press the focused button.
    expect(fireEvent.keyDown(screen.getByText("Close one"), { key: "Enter" })).toBe(true)
    expect(fireEvent.keyDown(screen.getByText("Close one"), { key: " " })).toBe(true)
    expect(onSelect).not.toHaveBeenCalled()
    fireEvent.keyDown(pill, { key: "Enter" })
    fireEvent.keyDown(pill, { key: " " })
    expect(onSelect).toHaveBeenCalledTimes(2)
  })

  it("walks the pills with the arrow keys", () => {
    strip()
    const [one, two] = screen.getAllByRole("tab")
    act(() => one.focus())
    fireEvent.keyDown(one, { key: "ArrowRight" })
    expect(document.activeElement).toBe(two)
  })

  it("runs a caller's handler beside the drag's listener of the same name, never instead of it", () => {
    const drag = vi.fn()
    const theirs = vi.fn()
    strip(vi.fn(), {
      sort: { setNodeRef: () => {}, style: {}, isDragging: false, listeners: { onMouseDown: drag } },
      onMouseDown: theirs,
    })
    fireEvent.mouseDown(screen.getAllByRole("tab")[0])
    expect(drag).toHaveBeenCalledTimes(1)
    expect(theirs).toHaveBeenCalledTimes(1)
  })

  it("refuses to render outside a TabStrip", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {})
    try {
      expect(() => render(<TabPill active onSelect={() => {}} icon={null} label="lost" />)).toThrow(
        /inside a TabStrip/,
      )
    } finally {
      quiet.mockRestore()
    }
  })
})

describe("TabStrip", () => {
  it("names a horizontal tab list holding the pills and none of the trailing controls", () => {
    strip()
    const list = screen.getByRole("tablist", { name: "Things" })
    expect(list.getAttribute("aria-orientation")).toBe("horizontal")
    expect(within(list).getAllByRole("tab")).toHaveLength(2)
    expect(within(list).queryByRole("button", { name: "Add" })).toBeNull()
    expect(screen.getByRole("button", { name: "Add" })).toBeTruthy()
  })
})
