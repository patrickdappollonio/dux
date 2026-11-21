import {
  DndContext,
  MouseSensor,
  TouchSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core"
import type {
  DragEndEvent,
  MouseSensorOptions,
  SensorProps,
  TouchSensorOptions,
} from "@dnd-kit/core"
import { SortableContext, horizontalListSortingStrategy } from "@dnd-kit/sortable"
import { useLayoutEffect, useRef, type ReactNode } from "react"

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
// dnd-kit has no way to cancel a drag from outside, but every sensor is handed
// its context's cancel, so these sensors pass it on to the strip that made them.
interface CancelSink {
  onCancellable?: (cancel: () => void) => void
}

class StripMouseSensor extends MouseSensor {
  constructor(props: SensorProps<MouseSensorOptions & CancelSink>) {
    super(props)
    props.options.onCancellable?.(props.onCancel)
  }
}

class StripTouchSensor extends TouchSensor {
  constructor(props: SensorProps<TouchSensorOptions & CancelSink>) {
    super(props)
    props.options.onCancellable?.(props.onCancel)
  }
}

export function SortableStrip({
  ids,
  onReorder,
  disabled = false,
  children,
}: {
  /** The pills' ids in their displayed order. */
  ids: string[]
  /** Called with the complete new order when a drop changes it. */
  onReorder: (next: string[]) => void
  /** No pill drags, a drag in flight is cancelled, and nothing is reordered. */
  disabled?: boolean
  children: ReactNode
}) {
  const cancelRef = useRef<(() => void) | null>(null)
  const onCancellable = (cancel: () => void) => {
    cancelRef.current = cancel
  }
  const settled = () => {
    cancelRef.current = null
  }
  const sensors = useSensors(
    useSensor(StripMouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION, onCancellable }),
    useSensor(StripTouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION, onCancellable }),
  )

  // A drag that outlives its strip's reorder would otherwise drop once the
  // strip is enabled again.
  useLayoutEffect(() => {
    if (!disabled) return
    const cancel = cancelRef.current
    cancelRef.current = null
    cancel?.()
  }, [disabled])

  function handleDragEnd({ active, over }: DragEndEvent) {
    settled()
    if (disabled || !over || active.id === over.id) return
    const next = moveItem(ids, String(active.id), String(over.id))
    if (!ordersMatch(ids, next)) onReorder(next)
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
      onDragCancel={settled}
      onDragAbort={settled}
    >
      <SortableContext
        items={ids}
        strategy={horizontalListSortingStrategy}
        disabled={disabled}
      >
        {children}
      </SortableContext>
    </DndContext>
  )
}
