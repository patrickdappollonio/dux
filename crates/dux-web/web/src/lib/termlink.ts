/**
 * Tapping an OSC 8 hyperlink on a touchscreen.
 *
 * On a phone the compose bar owns typing, so `TerminalPane`'s `touchend`
 * handler `preventDefault`s a plain tap and focuses the compose textarea
 * instead of letting xterm grab focus. `preventDefault` on `touchend` also
 * suppresses the browser's SYNTHETIC mouse events, and those are the only
 * thing that can activate a link: xterm's `Linkifier` resolves the hovered
 * link from `mousemove` and fires `link.activate` from `mouseup` (see
 * `@xterm/xterm/src/browser/Linkifier.ts`, which binds exactly
 * mousemove/mousedown/mouseup). So the tap never became a mouseup and a link
 * under the finger was unreachable: the tap only scrolled the compose bar into
 * focus and raised the keyboard.
 *
 * xterm publishes no "what link is at this cell" query, and the OSC 8 uri
 * lives in an internal service (`IOscLinkService`) that is not reachable from
 * the public API, so dux cannot hit-test the point itself without either
 * touching private state or re-implementing OSC 8 range tracking. Instead it
 * DRIVES the Linkifier through its own public contract: dispatch the mouse
 * sequence the suppressed synthetic events would have delivered, and let
 * xterm decide whether a link was there. A link tap then opens through exactly
 * the same `linkHandler.activate` a desktop click takes (and therefore the
 * same `linkActivateAction` gating and the same `noopener,noreferrer`
 * `window.open`), and an ordinary tap costs one scan of one buffer line.
 *
 * Two details make that safe rather than a shotgun replay of a click:
 *
 *  - The events go to the `.xterm-screen` element, which is what xterm hands
 *    the Linkifier, and they are dispatched with `bubbles: false`. Everything
 *    ELSE xterm does with a mouse (its focus grab, its selection service, and
 *    its mouse-report forwarding to the PTY) is bound one level up on
 *    `Terminal.element`, so a non-bubbling event cannot reach any of it. The
 *    compose-bar focus redirect and dux's own synthetic SGR click therefore
 *    stay exactly as they were.
 *  - A trailing `mouseleave` returns the Linkifier to its resting state, so a
 *    tapped link is not left underlined with a pointer cursor, and the next
 *    tap re-resolves rather than reading a cell cache that a repaint may have
 *    invalidated.
 */

/** The screen element xterm binds its Linkifier to, or null when not open. */
export function linkifierElement(root: HTMLElement | null | undefined): HTMLElement | null {
  return root?.querySelector<HTMLElement>(".xterm-screen") ?? null
}

/**
 * Replays the mouse sequence a tap would have produced, straight at the
 * Linkifier, and reports whether it activated a link.
 *
 * `activations` is a counter the pane bumps inside its `linkHandler.activate`
 * whenever it actually opens a tab; comparing it across the dispatch is what
 * tells a link tap from an ordinary one WITHOUT duplicating the open logic or
 * inspecting xterm's internals.
 *
 * `button: 0` and `detail: 1` matter: `linkActivateAction` refuses a
 * non-primary button and refuses the tail of a multi-click gesture, so a
 * sequence claiming anything else would be filtered out as not-a-click.
 */
export function activateLinkAtPoint(
  screen: HTMLElement | null,
  clientX: number,
  clientY: number,
  activations: () => number,
): boolean {
  if (!screen) return false
  const before = activations()
  // No `view`: xterm resolves a cell from `clientX`/`clientY` alone
  // (`MouseService.getCoords` takes exactly those two fields) and reads the
  // window from its own services, so a viewless event is enough.
  const mouse = (type: string, buttons: number, x = clientX, y = clientY) =>
    new MouseEvent(type, {
      bubbles: false,
      cancelable: true,
      clientX: x,
      clientY: y,
      button: 0,
      buttons,
      detail: 1,
    })
  // A PRIMING move at a different cell first. xterm's Linkifier only re-runs
  // its providers when the pointer's CELL changes from the last one it saw, and
  // a finger that taps the same link twice reports the same cell both times: on
  // the second tap the hover was skipped, no link was resolved, and the tap
  // opened nothing. MEASURED in the container as "a second tap on the same link
  // opens 0 tabs" before this line existed. Priming from the far side of the
  // element on the same row guarantees a different column while staying inside
  // the element (a point outside it resolves to no cell at all, which would
  // leave the stale cell in place instead of displacing it).
  const rect = screen.getBoundingClientRect()
  const primeX =
    clientX >= rect.left + rect.width / 2 ? rect.left + 1 : rect.right - 1
  screen.dispatchEvent(mouse("mousemove", 0, primeX, clientY))
  // Hover resolves the link under the point, down arms the Linkifier's
  // press/release pairing, up is what activates.
  screen.dispatchEvent(mouse("mousemove", 0))
  screen.dispatchEvent(mouse("mousedown", 1))
  screen.dispatchEvent(mouse("mouseup", 0))
  const activated = activations() > before
  // Back to rest: drops the hover underline and the pointer cursor a finger
  // leaves behind, and clears the cached link so the next tap re-resolves.
  screen.dispatchEvent(mouse("mouseleave", 0))
  return activated
}

/** What the rest of a tap should do once the link question is settled. */
export interface TerminalTapOutcome {
  /** Forward the tap to a mouse-tracking app as a synthetic SGR click. */
  forwardClick: boolean
  /** Move focus to the compose textarea (which raises the soft keyboard). */
  focusCompose: boolean
}

/** The runtime facts a tap is judged against. */
export interface TerminalTapContext {
  /** A link under the finger was opened by `activateLinkAtPoint`. */
  linkActivated: boolean
  /** The app in the PTY has mouse reporting on (`mouseTrackingMode !== "none"`). */
  mouseTracking: boolean
}

/**
 * Decides what a tap does after the link probe.
 *
 * The SGR click is INDEPENDENT of the link, deliberately. On a desktop the two
 * paths are bound to different elements and both run, so clicking a link
 * inside a mouse-tracking TUI both reports the click to the app and opens the
 * tab; a touch tap keeps that behaviour rather than inventing a rule where a
 * link swallows the click.
 *
 * Focus is the opposite: a link tap does NOT pull the caret into the compose
 * box. The gesture asked to go somewhere else (the tab is already opening),
 * and raising the soft keyboard over a terminal the user is leaving costs half
 * the screen for a message they did not start writing. It also matches the
 * desktop click, which moves no caret into a message box either. An ordinary
 * tap is unchanged and still focuses compose.
 */
export function terminalTapAction(ctx: TerminalTapContext): TerminalTapOutcome {
  return {
    forwardClick: ctx.mouseTracking,
    focusCompose: !ctx.linkActivated,
  }
}
