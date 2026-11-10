// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"
import type { DragEndEvent } from "@dnd-kit/core"

import { usePillSort } from "@/hooks/use-pill-sort"
import { SortableStrip } from "./SortableStrip"

// dnd-kit cannot run a real drag under jsdom (it measures rects), so the context
// is a passthrough that captures `onDragEnd` and the tests call it with
// synthetic events. What these pin is this strip's own part: which order a drop
// reports, and that a pill stays on the strip's axis.
let onDragEnd: (event: DragEndEvent) => void = () => {}
vi.mock("@dnd-kit/core", () => ({
  DndContext: ({
    children,
    onDragEnd: handler,
  }: {
    children: React.ReactNode
    onDragEnd: (event: DragEndEvent) => void
  }) => {
    onDragEnd = handler
    return children
  },
  MouseSensor: class {},
  TouchSensor: class {},
  closestCenter: () => {},
  useSensor: () => ({}),
  useSensors: (...sensors: unknown[]) => sensors,
}))
vi.mock("@dnd-kit/sortable", () => ({
  SortableContext: ({ children }: { children: React.ReactNode }) => children,
  horizontalListSortingStrategy: {},
  useSortable: () => ({
    listeners: {},
    setNodeRef: () => {},
    transform: { x: 12.4, y: 30, scaleX: 1, scaleY: 1 },
    transition: undefined,
    isDragging: false,
  }),
}))

function Pill({ id }: { id: string }) {
  const { setNodeRef, style, listeners } = usePillSort(id)
  return (
    <div ref={setNodeRef} style={style} {...listeners} data-testid={id}>
      {id}
    </div>
  )
}

function drop(active: string, over: string) {
  onDragEnd({ active: { id: active }, over: { id: over } } as DragEndEvent)
}

afterEach(cleanup)

describe("SortableStrip", () => {
  it("reports the whole new order when a pill is dropped on another's place", () => {
    const onReorder = vi.fn()
    render(
      <SortableStrip ids={["a", "b", "c"]} onReorder={onReorder}>
        <Pill id="a" />
      </SortableStrip>,
    )
    drop("a", "c")
    expect(onReorder).toHaveBeenCalledWith(["b", "c", "a"])
  })

  it("reports nothing for a drop that changes nothing", () => {
    const onReorder = vi.fn()
    render(
      <SortableStrip ids={["a", "b"]} onReorder={onReorder}>
        <Pill id="a" />
      </SortableStrip>,
    )
    drop("a", "a")
    onDragEnd({ active: { id: "a" }, over: null } as DragEndEvent)
    expect(onReorder).not.toHaveBeenCalled()
  })

  it("keeps a dragged pill on the strip's own axis", () => {
    render(
      <SortableStrip ids={["a"]} onReorder={() => {}}>
        <Pill id="a" />
      </SortableStrip>,
    )
    expect(screen.getByTestId("a").style.transform).toBe("translate3d(12px, 0, 0)")
  })
})
