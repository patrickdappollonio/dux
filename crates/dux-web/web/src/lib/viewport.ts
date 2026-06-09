// Heuristic for "is the soft keyboard open" on mobile. The keyboard shrinks the
// VISUAL viewport (window.visualViewport.height) but not the LAYOUT viewport
// (window.innerHeight), so a large gap between the two means the keyboard is up.
//
// The threshold sits ABOVE the iOS dynamic-toolbar (URL bar) collapse delta
// (~60-90px, which must NOT be mistaken for a keyboard) and BELOW the smallest
// real soft keyboard (~120px+ including its accessory row). 100px threads that
// gap. Tune here if a device misreports; pure so it can be unit-tested.
export const KEYBOARD_OPEN_THRESHOLD_PX = 100

export function keyboardLikelyOpen(
  viewportHeight: number,
  innerHeight: number,
): boolean {
  return innerHeight - viewportHeight > KEYBOARD_OPEN_THRESHOLD_PX
}
