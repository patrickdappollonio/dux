/**
 * Tapping an OSC 8 hyperlink on a touchscreen.
 *
 * A tap's `preventDefault` on `touchend` (the compose bar owns typing on a
 * phone) also suppresses the browser's synthetic mouse events, and those are the
 * only thing that can activate a link: xterm's `Linkifier` resolves the hovered
 * link from `mousemove` and fires `link.activate` from `mouseup`.
 *
 * xterm publishes no "what link is at this cell" query and the OSC 8 uri lives in
 * an internal service, so dux drives the Linkifier through its public contract
 * instead: dispatch the mouse sequence the suppressed synthetic events would have
 * delivered, and let xterm decide whether a link was there. A link tap then opens
 * through the same `linkHandler.activate` a desktop click takes, with the same
 * `linkActivateAction` gating.
 *
 * Two details make that safe rather than a shotgun replay of a click:
 *
 *  - The events go to `.xterm-screen` with `bubbles: false`. Everything else
 *    xterm does with a mouse (its focus grab, its selection service, its
 *    mouse-report forwarding) is bound one level up on `Terminal.element`, which
 *    a non-bubbling event cannot reach.
 *  - A trailing `mouseleave` returns the Linkifier to rest, so a tapped link is
 *    not left underlined and the next tap re-resolves rather than reading a cell
 *    cache a repaint may have invalidated.
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
 * `activations` is a counter the link-press machine bumps inside its one opener
 * (`openLink`) whenever it actually opens a tab; comparing it across the dispatch
 * tells a link tap from an ordinary one without duplicating the open logic or
 * inspecting xterm's internals.
 *
 * `button: 0` and `detail: 1` matter: `linkActivateAction` refuses a non-primary
 * button and the tail of a multi-click gesture, so a sequence claiming anything
 * else is filtered out as not-a-click.
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
  // Sampled after the hover and before the press, so only the press/release pair
  // can count as a hit, never either of the resets around it.
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
 * No `view`: xterm resolves a cell from `clientX`/`clientY` alone and reads the
 * window from its own services, so a viewless event is enough.
 *
 * Every event is tagged as a dux replay. The pane's capture-phase link intercept
 * sits above this element and runs even for a `bubbles: false` dispatch, so
 * without the tag dux's own probe would be judged as a human press. `isTrusted`
 * cannot do that job; see `lib/termreplay.ts`.
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
 * scroll under a stationary pointer, a page's first click may follow no mousemove
 * at all, and a resize clears the Linkifier's current link, each leaking either a
 * server-side open or a stale true that swallows a TUI button press. The whole
 * chain is synchronous in the installed xterm 6, so the hover ref is up to date
 * by the time this returns.
 *
 * Two properties are load-bearing and must not be tidied: `bubbles: false`, and
 * dispatch at `.xterm-screen` rather than `Terminal.element`, where xterm's
 * mouse-report listener lives. A bubbling or element-targeted move would be
 * encoded and sent to the app, giving an any-motion (1003) app two motion reports
 * per click, one at a fabricated cell on the far side of the row.
 *
 * Accepted cost: the far-side prime can hover, then leave, a different link
 * sharing the row. The ref is correct either way (last write wins, in the same
 * tick) and the cost is at worst a one-frame underline flicker.
 */
export function primeLinkHover(
  screen: HTMLElement | null,
  clientX: number,
  clientY: number,
): void {
  if (!screen) return
  const mouse = linkMouseEvent(clientX, clientY)
  // Start from rest. `mouseleave` makes xterm drop its current link and empty the
  // caller's hover record, so a point that resolves to no cell at all leaves "no
  // link here" behind rather than the last link the pointer touched. The moves
  // below re-resolve from xterm's per-line cache, so this costs a lookup, not a
  // repaint.
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
 * Focus is the opposite: a link tap does not pull the caret into the compose box,
 * because raising the soft keyboard over a terminal the user is leaving costs
 * half the screen for a message they did not start writing, and the desktop click
 * moves no caret either. An ordinary tap still focuses compose.
 */
export function terminalTapAction(ctx: TerminalTapContext): TerminalTapOutcome {
  return {
    forwardClick: ctx.mouseTracking && !ctx.linkActivated,
    focusCompose: !ctx.linkActivated,
  }
}
