import { useSortable } from "@dnd-kit/sortable"
import type { CSSProperties } from "react"

// What one pill spreads onto its element to be draggable inside a
// `SortableStrip`. The drag's accessibility attributes are left out on purpose:
// the pill is already a `tab`, and a drag handle role on it would replace that.
export function usePillSort(id: string): {
  setNodeRef: (node: HTMLElement | null) => void
  style: CSSProperties
  listeners: ReturnType<typeof useSortable>["listeners"]
  isDragging: boolean
} {
  const { listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id })
  return {
    setNodeRef,
    listeners,
    isDragging,
    style: {
      // Locked to the strip's own axis: a pill dragged sideways must not drift
      // off the strip vertically.
      transform: transform ? `translate3d(${Math.round(transform.x)}px, 0, 0)` : undefined,
      transition,
      opacity: isDragging ? 0.6 : undefined,
    },
  }
}
