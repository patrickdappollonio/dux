import {
  DndContext,
  MouseSensor,
  TouchSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core"
import type { DragEndEvent } from "@dnd-kit/core"
import { SortableContext, horizontalListSortingStrategy } from "@dnd-kit/sortable"
import type { ReactNode } from "react"

import { MOUSE_DRAG_ACTIVATION, TOUCH_DRAG_ACTIVATION } from "@/lib/dragActivation"
import { moveItem, ordersMatch } from "@/lib/reorder"

// The drag half of a pill strip, shared by every strip whose pills reorder along
// themselves: wrap the pills in `SortableStrip` and give each one `usePillSort`
// (`hooks/use-pill-sort.ts`).
// It renders no element of its own, so the strip keeps its own container and
// layout. A strip reorders within itself only: it is its own DndContext holding
// only its own ids, so a pill can never be dropped on another list.
//
// Mouse drags on the sidebar's small pull and touch drags on a hold, from
// `lib/dragActivation.ts`, so a plain press is still a select.
export function SortableStrip({
  ids,
  onReorder,
  children,
}: {
  /** The pills' ids in their displayed order. */
  ids: string[]
  /** Called with the complete new order when a drop changes it. */
  onReorder: (next: string[]) => void
  children: ReactNode
}) {
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION }),
    useSensor(TouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION }),
  )

  function handleDragEnd({ active, over }: DragEndEvent) {
    if (!over || active.id === over.id) return
    const next = moveItem(ids, String(active.id), String(over.id))
    if (!ordersMatch(ids, next)) onReorder(next)
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={ids} strategy={horizontalListSortingStrategy}>
        {children}
      </SortableContext>
    </DndContext>
  )
}
