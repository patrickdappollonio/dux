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

import { markDuxReplay } from "./termreplay"

/** The screen element xterm binds its Linkifier to, or null when not open. */
export function linkifierElement(root: HTMLElement | null | undefined): HTMLElement | null {
  return root?.querySelector<HTMLElement>(".xterm-screen") ?? null
}

/**
 * Replays the mouse sequence a tap would have produced, straight at the
 * Linkifier, and reports whether it activated a link.
 *
 * `activations` is a counter the link-press machine bumps inside its one opener (`openLink`), whichever client triggered it
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
  const mouse = linkMouseEvent(clientX, clientY)
  // Hover resolves the link under the point, down arms the Linkifier's
  // press/release pairing, up is what activates.
  primeLinkHover(screen, clientX, clientY)
  // Sampled AFTER the hover and before the press, so only the press/release
  // pair can count as a hit: neither the reset that opens the prime nor the one
  // that closes this replay may fake one.
  const before = activations()
  screen.dispatchEvent(mouse("mousedown", 1))
  screen.dispatchEvent(mouse("mouseup", 0))
  const activated = activations() > before
  // Back to rest: drops the hover underline and the pointer cursor a finger
  // leaves behind, and clears the cached link so the next tap re-resolves.
  screen.dispatchEvent(mouse("mouseleave", 0))
  return activated
}

/**
 * Builds one event of a link replay at a fixed point.
 *
 * No `view`: xterm resolves a cell from `clientX`/`clientY` alone
 * (`MouseService.getCoords` takes exactly those two fields) and reads the
 * window from its own services, so a viewless event is enough.
 *
 * Every event is TAGGED as a dux replay. The pane's capture-phase link
 * intercept sits above this element and a capture listener runs even for a
 * `bubbles: false` dispatch, so without the tag dux's own probe would be judged
 * as if a human had pressed the mouse. `isTrusted` cannot do that job; see
 * `lib/termreplay.ts`.
 */
function linkMouseEvent(clientX: number, clientY: number) {
  return (type: string, buttons: number, x = clientX, y = clientY) =>
    markDuxReplay(
      new MouseEvent(type, {
        bubbles: false,
        cancelable: true,
        clientX: x,
        clientY: y,
        button: 0,
        buttons,
        detail: 1,
      }),
    )
}

/**
 * Resolves the link under a point through xterm's own Linkifier, synchronously,
 * without pressing anything.
 *
 * This is how the desktop press-time decision learns whether the pointer is on
 * an OSC 8 link. Passive hover tracking alone cannot answer it: the buffer can
 * scroll under a stationary pointer, the first click of a page may follow no
 * mousemove at all, and a resize clears the Linkifier's current link. Each of
 * those leaks either a server-side open or (worse) a stale true that swallows a
 * TUI button press. Driving the Linkifier at the moment of the press is what
 * makes the answer truthful, and the whole chain (the OSC link provider's
 * `provideLinks`, its callback, and `linkHandler.hover`) is synchronous in the
 * installed xterm 6, so the hover ref is up to date by the time this returns.
 *
 * TWO PROPERTIES ARE LOAD-BEARING and must not be "tidied":
 *
 *  - `bubbles: false`, and
 *  - dispatched at `.xterm-screen`, not at `Terminal.element`.
 *
 * xterm's mouse-report listener lives one level UP, on `Terminal.element`. A
 * bubbling or element-targeted move would therefore be encoded and sent to the
 * app: harmless under DECSET 1000/1002, but an any-motion (1003) app would
 * receive two MOTION reports per click, one of them at a fabricated cell on the
 * far side of the row.
 *
 * NIT, accepted: the far-side prime can hover, then leave, a DIFFERENT link
 * sharing the row. The ref is correct either way (last write wins, all in the
 * same tick) and the only cost is a possible one-frame underline flicker.
 */
export function primeLinkHover(
  screen: HTMLElement | null,
  clientX: number,
  clientY: number,
): void {
  if (!screen) return
  const mouse = linkMouseEvent(clientX, clientY)
  // Start from REST. `mouseleave` makes xterm drop its current link, which
  // fires `leave` and empties the caller's hover record, so a point that
  // resolves to no cell at all (a press in the pane's padding, or a buffer that
  // scrolled out from under a still pointer) leaves "no link here" behind
  // rather than the last link the pointer happened to touch. The moves below
  // then re-resolve from xterm's own per-line cache, so this costs a lookup,
  // not a repaint.
  screen.dispatchEvent(mouse("mouseleave", 0))
  // Prime a different in-bounds cell because xterm's Linkifier reruns providers
  // only when the pointer cell changes. This makes repeated taps resolve again.
  const rect = screen.getBoundingClientRect()
  const primeX =
    clientX >= rect.left + rect.width / 2 ? rect.left + 1 : rect.right - 1
  screen.dispatchEvent(mouse("mousemove", 0, primeX, clientY))
  screen.dispatchEvent(mouse("mousemove", 0))
}

/** What the rest of a tap should do once the link question is settled. */
export interface TerminalTapOutcome {
  /** Forward the tap to a mouse-tracking app as a replayed click. */
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
 * A tap that opens a link belongs to dux and is not forwarded to the terminal;
 * otherwise a mouse-aware remote app could open the same URL on the server.
 * Touch has no force-forward chord because long press already means local use.
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
    forwardClick: ctx.mouseTracking && !ctx.linkActivated,
    focusCompose: !ctx.linkActivated,
  }
}
