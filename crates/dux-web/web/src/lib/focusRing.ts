/**
 * The keyboard focus ring the shared `Button` draws, for a control that cannot
 * be a `Button` but must look focused the same way. The border half needs the
 * element to carry a (transparent) 1px border for `focus-visible:border-ring` to
 * colour; the ring half is a box-shadow, so any ancestor that clips its overflow
 * shears it off and must let it out while the control holds keyboard focus.
 */
export const FOCUS_RING =
  "outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
