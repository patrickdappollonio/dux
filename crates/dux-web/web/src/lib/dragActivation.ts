// Activation constraints for the sidebar/hub reorder drags (the agents list
// and the terminals list share them through FlatAgentList's one `useSensors`
// call). Kept as plain data in a leaf module so the values are unit-testable
// without mounting dnd-kit.
//
// Two sensors instead of the previous single PointerSensor, because
// @dnd-kit/core 6.3.1's `PointerActivationConstraint` applies one constraint
// to every pointer type (its `DelayConstraint | DistanceConstraint` union has
// no per-pointer-type branch, read from the installed package's
// AbstractPointerSensor.d.ts), and the two input kinds need OPPOSITE gates:
//
// - MOUSE keeps the small distance gate: a plain click stays a select, and a
//   6px pull starts the drag immediately, exactly the previous desktop feel.
// - TOUCH arms on a HOLD (delay + tolerance): with the old instant
//   activation, a touch drag armed on contact and fought the list's own
//   scroll gesture, so on phones reordering "briefly glitched" and aborted.
//   The delay makes a swipe scroll (moving past the tolerance during the
//   hold cancels activation) and a deliberate hold grab the row. The
//   activator buttons carry `touch-manipulation`, the touch-action dnd-kit
//   pairs with delayed touch activation (`none` would kill list scrolling
//   entirely; once the drag IS active the TouchSensor's non-passive touchmove
//   listener prevents scrolling for the drag's duration).
//
// The hold length sits between "a scroll-intent touch still arms it"
// (~200ms and below) and the browser's own long-press behaviors (~500ms).

export const MOUSE_DRAG_ACTIVATION = { distance: 6 } as const

export const TOUCH_DRAG_ACTIVATION = { delay: 300, tolerance: 8 } as const
