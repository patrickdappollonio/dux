// WHICH ITEMS THE INPUT ⋯ MENU CARRIES, as data rather than as rendering.
//
// The gates live here, away from the component, because two different callers
// answer them differently (the input ⋯ below the terminal widens the keys item
// to a coarse-pointer tablet; the phone header menus keep the narrower shell
// gate) and because the ANCHOR has to ask "would this menu be empty?" before it
// decides whether to render a trigger at all.
//
// THESE TWO ROWS ARE THE ONES EITHER KIND OF MENU CAN CARRY, and exactly one of
// the two carries either of them at a time (see `paneInputGroup.ts`). "Attach a
// file…" and the theater exit are deliberately NOT gates: each has one home,
// the top menu's INPUT group, and the flag for it belongs to the caller that
// also supplies the act behind it. Modelling them here left the bottom menu
// holding two fields it could only ever set to `false`.

/** Which of the two shared rows a caller wants. Each is the caller's predicate. */
export interface InputMenuGates {
  /// The typing-surface switch. See `inputMenuSurfaceSwitchOffered`.
  surfaceSwitch: boolean
  /// Hide/Show terminal keys (`ui.mobile_accessory_bar`).
  keysToggle: boolean
}

/**
 * Would the input `⋯` render anything at all?
 *
 * An `⋯` that opens an empty popup is worse than no `⋯`, and the empty state is
 * genuinely reachable (a fine pointer with the compose bar up through a stored
 * choice, desktop shell). The anchor asks this before rendering its trigger.
 */
export function inputMenuHasItems(gates: InputMenuGates): boolean {
  return gates.surfaceSwitch || gates.keysToggle
}
