// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { usePillSort } from "@/hooks/use-pill-sort"
import { SortableStrip } from "./SortableStrip"

// A real dnd-kit drag, which `SortableStrip.test.tsx` mocks away.

function Pill({ id }: { id: string }) {
  const { setNodeRef, style, listeners } = usePillSort(id)
  return (
    <div ref={setNodeRef} style={style} {...listeners} data-testid={id}>
      {id}
    </div>
  )
}

function Strip({ disabled, onReorder }: { disabled: boolean; onReorder: (n: string[]) => void }) {
  return (
    <SortableStrip ids={["a", "b"]} onReorder={onReorder} disabled={disabled}>
      <Pill id="a" />
      <Pill id="b" />
    </SortableStrip>
  )
}

afterEach(cleanup)

describe("SortableStrip drag in flight", () => {
  it("cancels the drag when the strip's reorder becomes disabled", () => {
    const onReorder = vi.fn()
    const { rerender } = render(<Strip disabled={false} onReorder={onReorder} />)
    const pill = screen.getByTestId("b")
    fireEvent.mouseDown(pill, { button: 0, clientX: 50, clientY: 10 })
    fireEvent.mouseMove(document, { clientX: 40, clientY: 10 })
    fireEvent.mouseMove(document, { clientX: 20, clientY: 10 })
    expect(pill.style.transform).not.toBe("")
    rerender(<Strip disabled onReorder={onReorder} />)
    expect(pill.style.transform).toBe("")
    fireEvent.mouseMove(document, { clientX: 0, clientY: 10 })
    expect(pill.style.transform).toBe("")
    // The page may drive again before the pointer is released.
    rerender(<Strip disabled={false} onReorder={onReorder} />)
    act(() => {
      fireEvent.mouseUp(document, { clientX: 0, clientY: 10 })
    })
    expect(onReorder).not.toHaveBeenCalled()
  })
})
