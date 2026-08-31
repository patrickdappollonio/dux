// WHICH ITEMS THE INPUT ⋯ MENU CARRIES, as data rather than as rendering.
//
// The gates live here, away from the component, because two different callers
// answer them differently (the input ⋯ below the terminal widens the keys item
// to a coarse-pointer tablet; the phone header menus keep the narrower shell
// gate) and because the ANCHORS have to ask "would this menu be empty?" before
// they decide whether to render a row at all.

/** Which items a caller wants. Every field is the caller's own predicate. */
export interface InputMenuGates {
  /// "Attach a file…". Off when uploads are switched off server-side
  /// (`file_drop_max_bytes = 0`) and for anyone who does not own the input:
  /// a non-owner cannot paste the saved path afterwards, so the file would
  /// strand.
  attach: boolean
  /// The typing-surface switch. See `inputMenuSurfaceSwitchOffered`.
  surfaceSwitch: boolean
  /// Hide/Show terminal keys (`ui.mobile_accessory_bar`).
  keysToggle: boolean
  /// Hide/Show top bar (`ui.mobile_top_bar`). Phone shell only: the top bar is
  /// the mobile shell's own chrome and does not exist in the wide layout.
  topBarToggle: boolean
  /// "Leave theater mode". Only while theater is actually on, because this is a
  /// way BACK and not a way there: the header's expand button is the way there,
  /// and in theater that header is exactly what is not on screen. This menu is
  /// the guaranteed exit on a phone, where the floating pill can end up under
  /// the soft keyboard.
  theaterExit: boolean
}

/**
 * Would `InputMenuItems` render anything at all?
 *
 * An `⋯` that opens an empty popup is worse than no `⋯`, and the empty state is
 * genuinely reachable (a fine pointer with the compose bar up through a stored
 * choice, uploads switched off, desktop shell). Every anchor asks this before
 * rendering its trigger.
 */
export function inputMenuHasItems(gates: InputMenuGates): boolean {
  return (
    gates.attach ||
    gates.surfaceSwitch ||
    gates.keysToggle ||
    gates.topBarToggle ||
    gates.theaterExit
  )
}
