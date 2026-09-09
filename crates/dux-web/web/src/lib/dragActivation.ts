// Activation constraints for the sidebar/hub reorder drags, shared by the agents and terminals
// lists through FlatAgentList's one `useSensors` call.
//
// Two sensors, one per pointer kind, because @dnd-kit/core's `PointerActivationConstraint`
// applies one constraint to every pointer type, and the two kinds need opposite gates:
//
// - Mouse keeps a small distance gate: a plain click stays a select, a short pull drags.
// - Touch arms on a hold, because instant activation fights the list's own scroll gesture.
//   Moving past the tolerance during the hold cancels activation, so a swipe still scrolls.
//   The activator buttons carry `touch-manipulation`, the touch-action dnd-kit pairs with
//   delayed touch activation; `none` would kill list scrolling entirely.
//
// The hold sits between "a scroll-intent touch still arms it" (~200ms) and the browser's own
// long-press behaviors (~500ms).

export const MOUSE_DRAG_ACTIVATION = { distance: 6 } as const

export const TOUCH_DRAG_ACTIVATION = { delay: 300, tolerance: 8 } as const
