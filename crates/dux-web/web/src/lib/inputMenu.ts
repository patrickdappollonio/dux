// Which items the input ⋯ menu carries, as data rather than rendering: callers answer
// the gates differently, and an anchor must ask whether the menu would be empty
// before it renders a trigger at all.
//
// These are the rows either kind of menu can carry, and one menu carries either of
// them at a time (see `paneInputGroup.ts`). "Attach a file…" and the theater exit are
// deliberately not gates: each has one home in the top menu's input group, and its
// flag belongs to the caller that also supplies the act behind it.

/** Which of the two shared rows a caller wants. Each is the caller's predicate. */
export interface InputMenuGates {
  /// The typing-surface switch. See `inputMenuSurfaceSwitchOffered`.
  surfaceSwitch: boolean
  /// Hide/Show terminal keys (`ui.mobile_accessory_bar`).
  keysToggle: boolean
}

/**
 * Would the input `⋯` render anything at all? The anchor asks before rendering its
 * trigger, because an `⋯` that opens an empty popup is worse than no `⋯`.
 */
export function inputMenuHasItems(gates: InputMenuGates): boolean {
  return gates.surfaceSwitch || gates.keysToggle
}
