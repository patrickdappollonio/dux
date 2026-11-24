// What each placement of the shared `⋯` trigger looks like. Kept apart from the
// component so a neighbour that must match its box can import it.

/**
 * Where the `⋯` sits, which decides its look and its box. A caller names a
 * place and never writes a size, a border or a touch rule.
 *
 * - `row`: a list row's trailing action (ghost, 32px).
 * - `toolbar`: a ghost button among small toolbar buttons (28px).
 * - `header`: a pane, bar or form header's outline button (32px).
 * - `banner`: a strip's flush trailing control, at the strip's height token.
 * - `floating`: a round 40px button on the flap or the floating pill. These two
 *   fly as one object, so this box never changes.
 * - `tab-pill`: inside a tab strip's pill (`TabPill`), which is shorter than
 *   40px on a phone. Every strip's pills are `TabPill`, the dock's included.
 *
 * Under a coarse pointer, and on a phone, every placement reaches the 40px
 * floor on the axis where its neighbours could take a stray tap; the pills keep
 * a 20px box with an invisible margin on a tablet, and a strip-height box on a
 * phone, because their neighbour on the other axis is the next tab.
 */
export type EllipsisPlacement =
  | "row"
  | "toolbar"
  | "header"
  | "banner"
  | "floating"
  | "tab-pill"

export type PillPlacement = "tab-pill"

export function isPillPlacement(p: EllipsisPlacement): p is PillPlacement {
  return p === "tab-pill"
}

/**
 * How a row's `⋯` is fitted to its row; each is one row's reason, and none
 * combine.
 *
 * - `compact`: 28px under a fine pointer, for the sidebar's agent and terminal
 *   rows, where the only neighbour on either axis is the row's own button,
 *   whose click selects the agent or terminal this menu is about.
 * - `slim`: narrower on a phone under a finger, keeping the 40px floor in
 *   height, for a row whose title needs the width. It sits between the row's
 *   text and the screen's edge, and above and below are the rows' own
 *   full-height controls. Its box is `ROW_BOX.slim`, which the grip beside it
 *   shares.
 * - `roomy`: a phone's 44px box, for a row whose neighbours already have it,
 *   so the controls of one cluster share a height.
 */
export const ROW_BOX = {
  // 32px under a fine pointer, where the only neighbour on either axis is the
  // row's own button; the 40px floor returns on a phone and any coarse pointer.
  standard: "size-8 shrink-0 max-md:size-10 pointer-coarse:size-10",
  compact: "size-7 shrink-0 max-md:size-10 pointer-coarse:size-10",
  slim: "size-8 shrink-0 pointer-coarse:size-10 max-md:pointer-coarse:w-7",
  roomy:
    "size-8 shrink-0 max-md:size-11 pointer-coarse:size-10 max-md:pointer-coarse:size-11",
} as const
export type RowFit = Exclude<keyof typeof ROW_BOX, "standard">

// A header's one icon-only box, shared by the `⋯` and by every icon-only button
// in a header (`HeaderButton`), which sit side by side and so share a height.
export const HEADER_BOX = "size-8 shrink-0 max-md:size-10 pointer-coarse:size-10"

/**
 * The banner strip's one height token. The counter and the controls beside the
 * strip's link carry it too, so they sit flush with the strip and with each
 * other; on a coarse pointer the whole strip grows to the 40px touch floor.
 */
export const BANNER_STRIP_HEIGHT = "h-9 pointer-coarse:h-10"

// A phone gets 32px high and the full 44px wide (its horizontal
// neighbour is the next tab), fitting inside the pill's 36px height;
// a 44px square would force the pill back to 44px. A tablet keeps the 20px
// box and an invisible 10px margin takes the tap area to 40px. Its left 10px
// lie over the pill's own label or pin, so a tap there opens the menu rather
// than selecting the tab.
const PILL_BOX =
  "relative flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground after:absolute after:-inset-2.5 after:hidden hover:text-foreground max-md:h-8 max-md:w-11 md:pointer-coarse:after:block"
const PILL_REVEAL_CAP = "[--reveal-cap:--spacing(8)] max-md:max-w-none"

export const BOX = {
  toolbar: "shrink-0 max-md:size-10 pointer-coarse:size-10",
  // The strip's height token, and the touch floor in width on a coarse pointer.
  // On a fine pointer the width stays the strip's 36px: above and below is the
  // terminal or the strip's own edge, nothing a stray press could land on, and
  // beside it sit the strip's own link and its edge.
  banner: `${BANNER_STRIP_HEIGHT} w-9 rounded-none text-current pointer-coarse:w-10`,
  floating: "size-10 shrink-0 rounded-full",
  "tab-pill": PILL_BOX,
} as const

/** The banner's control box, shared with the banner's previous and next buttons. */
export const BANNER_CONTROL_BOX = BOX.banner

// The reveal wrapper's width cap is the trigger's own width, so a revealed
// slot opens exactly as wide as what it holds. Only a row and a pill reveal.
export const REVEAL_CAP: Record<"row" | PillPlacement, string> = {
  row: "[--reveal-cap:--spacing(10)]",
  "tab-pill": PILL_REVEAL_CAP,
}
