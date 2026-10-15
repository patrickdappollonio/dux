/**
 * The keyboard focus ring the shared `Button` draws, for a control that cannot
 * be a `Button` but must look focused the same way. The border half needs the
 * element to carry a (transparent) 1px border for `focus-visible:border-ring` to
 * colour; the ring half is a box-shadow, so any ancestor that clips its overflow
 * shears it off and must let it out while the control holds keyboard focus.
 */
export const FOCUS_RING =
  "outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"

/**
 * The same ring, drawn whatever has focus, for the row a search field's Enter
 * would pick while the field keeps keyboard focus. It is drawn INSIDE the row
 * (an inset ring for the soft half, a pulled-in outline for the border half),
 * because the lists it lights clip their overflow, and it is a ring rather than
 * a background so it never reads as the selected or open row, which already
 * wear the hover background.
 */
export const ACTIVE_ROW_RING =
  "inset-ring-3 inset-ring-ring/50 outline-1 -outline-offset-1 outline-ring"
