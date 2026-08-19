// THE LINK-PRESS MACHINE.
//
// ONE CLICK ON A HYPERLINK OPENS ONE TAB, ON THE CLICKER'S SIDE. This machine
// is the only thing in the pane that opens a terminal hyperlink, and it holds
// the three pieces that decision needs: the hover cache (the OSC 8 uri under
// the pointer, as xterm's own Linkifier resolves it), the in-flight press
// record, and the activation counter.
//
// THREE DECLARED CLIENTS read it, and there are no others:
//   1. xterm's own Linkifier, through the `linkHandler` this exposes: the
//      tracking-OFF path, where xterm hit-tests and activates by itself.
//   2. the capture-phase release below: the tracking-ON path, where dux
//      swallowed the press and must open the link itself.
//   3. the TOUCH TAP probe, which replays a synthetic hover-and-click at the
//      Linkifier and compares `activations()` across it to learn whether the
//      tap landed on a link at all.
//
// IT ABSTAINS UNDER THE FORCE-LOCAL-SELECTION MODIFIER, entirely: the press
// passes through, xterm forwards nothing under it, and dux opens nothing. The
// trap runs in both directions. Treating the other platform's modifier as
// force-selection leaves a press xterm WOULD forward unsuppressed (the
// server-side double open returns); swallowing it makes a link the one place
// the documented selection hatch fails, on exactly the text people select most.
// And since the press passes through, xterm's Linkifier still activates on the
// drag-end mouseup, so the ACTIVATE side refuses it too or a selection opens a
// tab. Both refusals live in the pure `termkeys` helpers this calls.
import type { Terminal } from "@xterm/xterm"

import {
  linkActivateAction,
  type LinkActivateEvent,
  linkPressAction,
  linkReleaseOpens,
} from "@/lib/termkeys"
import { linkifierElement, primeLinkHover } from "@/lib/termlink"
import { isDuxReplay } from "@/lib/termreplay"

import { DRAG_THRESHOLD_PX } from "./constants"
import { raiseLinkForwardHint } from "./pageSessionHints"

export type LinkPressDeps = {
  /// `capabilities.hyperlinks`, read live so a toggle never recreates the
  /// terminal.
  hyperlinks: () => boolean
  /// Which chord is the force-forward hatch: Cmd on a Mac, Ctrl elsewhere.
  isMac: boolean
}

export type LinkPress = {
  /// Hand this to the `Terminal` constructor. `hover`/`leave` exist for the
  /// capture intercept, not for decoration: they are the only public read of
  /// "is there a link at this point", since the OSC 8 uri lives in an internal
  /// service.
  linkHandler: {
    activate: (event: MouseEvent, uri: string) => void
    hover: (event: unknown, uri: string) => void
    leave: () => void
  }
  /// The terminal, once it exists. The handler above has to be passed INTO the
  /// constructor, so the machine learns its terminal immediately afterwards.
  setTerminal: (term: Terminal) => void
  /// Register the capture-phase press and release on the container.
  attach: (container: Element) => void
  /// How many tabs this machine has opened. The touch probe compares it across
  /// its replay; nothing else may count opens.
  activations: () => number
  dispose: () => void
}

export function createLinkPress(deps: LinkPressDeps): LinkPress {
  const isMac = deps.isMac
  // Bumped every time a tab is actually opened.
  let linkActivations = 0
  // The OSC 8 link under the pointer, as xterm's own Linkifier resolves it.
  // Written by the `hover`/`leave` half of the link handler, and driven
  // deliberately at press time by `primeLinkHover` so the desktop intercept
  // reads a truthful value rather than whatever a passive mousemove last left
  // here (the buffer can scroll under a still pointer, the first click of a
  // page may follow no move at all, and a resize clears the current link).
  let hoveredLinkUri: string | null = null
  let terminal: Terminal | null = null
  let container: Element | null = null
  const term = (): Terminal => {
    if (terminal === null) throw new Error("link machine used before its terminal")
    return terminal
  }

  // THE ONE PLACE A TERMINAL HYPERLINK IS OPENED. Two call sites reach it:
  // xterm's Linkifier `activate` (the tracking-off path) and the capture-phase
  // intercept's release (the tracking-on path). They must stay identical in
  // every respect that is visible to the user, which is what a shared function
  // buys: the same `linkActivateAction` truth table, the same
  // `noopener,noreferrer` window, and the same activation counter the touch
  // probe reads.
  const openLink = (ev: LinkActivateEvent, uri: string): boolean => {
    const action = linkActivateAction(ev, {
      hyperlinks: deps.hyperlinks(),
      uri,
      mouseTracking: term().modes.mouseTrackingMode !== "none",
      isMac,
    })
    if (action !== "open") return false
    window.open(uri, "_blank", "noopener,noreferrer")
    linkActivations++
    return true
  }

  // ONE CLICK ON A HYPERLINK OPENS ONE TAB, ON THE CLICKER'S SIDE.
  //
  // With the app in the PTY tracking the mouse, a click on an OSC 8 link used
  // to open the page TWICE: dux's `window.open` in this browser, and the same
  // click forwarded as a mouse report, which an agent CLI answers by shelling
  // out to `open <url>` ON THE SERVER'S MACHINE. Only the first one reaches
  // the person who clicked, so dux is the sole opener and the click that
  // dispatched a link is withheld from the app entirely.
  //
  // dux DIVERGES from iTerm2, Ghostty and kitty here, deliberately: they give
  // a plain click to the app and reserve links for a modifier. That is the
  // right answer for a local terminal and the wrong one for a remote-first
  // tool, where the app's own open runs on a computer the user may never see.
  // The escape hatch is the chord in `linkHatchHeld`, which forwards the click
  // and refuses dux's open.
  //
  // MECHANISM (version-dependent, MEASURED against the installed xterm 6):
  // xterm feeds mouse reports from DOM MOUSE listeners only (there is no
  // pointer-event path) through one choke point, `bindMouse` in
  // `CoreBrowserTerminal`, and it registers the document-level release and
  // drag reporters INSIDE its element `mousedown` handler. So swallowing the
  // PRESS suppresses the whole report pair, including a release that happens
  // outside the pane, and nothing else needs suppressing out there.
  //
  // Everything about the shape of this is load-bearing:
  //  - CAPTURE phase on the CONTAINER, the same trick as the paste guard
  //    below: xterm's listeners are on descendants, so a capture listener here
  //    decides first, and `stopPropagation` (not `stopImmediatePropagation`,
  //    which would also silence dux's own bubble-phase copy-on-select) keeps
  //    the event from ever reaching them.
  //  - PRESS TIME, because xterm emits the press report from `mousedown`.
  //    Deciding at release would already have leaked a lone press, and
  //    press-activated TUI controls act on exactly that.
  //  - PRIMARY BUTTON ONLY, so right-click paste and every context menu path
  //    are untouched.
  //  - TRACKING ON ONLY. With no app capturing the mouse there is no report to
  //    suppress, and swallowing would cost the focus grab, the selection clear
  //    and the drag-select that starts on a link.
  //  - dux's OWN replays are tagged and skipped (`lib/termreplay.ts`); an
  //    `isTrusted` check would instead skip every test and every
  //    assistive-technology click.
  let inFlight: { uri: string; x: number; y: number; open: boolean } | null = null
  let outsideReleaseWatch: ((e: MouseEvent) => void) | null = null
  const disarmOutsideRelease = () => {
    if (!outsideReleaseWatch) return
    document.removeEventListener("mouseup", outsideReleaseWatch, true)
    outsideReleaseWatch = null
  }
  // A swallowed press may be released anywhere, including over another pane or
  // the desktop, and the in-flight record has to clear either way or the next
  // click reads a stale one. This OBSERVES AND CLEARS and never stops
  // propagation: xterm's own document reporter was never attached for a press
  // dux swallowed, so there is nothing out here to suppress, and a swallowing
  // one-shot would eat an unrelated mouseup after a release the window never
  // saw (an alt-tab away with the button down).
  const armOutsideRelease = () => {
    disarmOutsideRelease()
    const watch = (e: MouseEvent) => {
      // The PRIMARY release ends the gesture; a chorded right release while
      // the left button is still down is somebody else's event.
      if (e.button !== 0) return
      disarmOutsideRelease()
      // A release INSIDE the pane belongs to the capture handler below. This
      // one runs first (the document is the outermost node of the capture
      // path), so it must leave the record alone for it.
      if (e.target instanceof Node && container?.contains(e.target)) return
      inFlight = null
    }
    outsideReleaseWatch = watch
    document.addEventListener("mouseup", watch, true)
  }
  const onLinkPressCapture = (e: MouseEvent) => {
    if (isDuxReplay(e)) return
    // NON-PRIMARY BUTTONS MUST NOT TOUCH THE IN-FLIGHT RECORD, which is why
    // this gate comes before the reset below. A right press while the left
    // button is still down (chording a paste mid-gesture) would otherwise wipe
    // a swallowed press, and its left release would then be forwarded alone:
    // a release report for a gesture the app never saw begin.
    if (e.button !== 0) return
    // A new PRIMARY press ends the previous gesture whatever happened to it,
    // so a release the window never delivered cannot wedge the next click.
    inFlight = null
    disarmOutsideRelease()
    if (term().modes.mouseTrackingMode === "none") return
    // Resolve the link under the press SYNCHRONOUSLY through xterm's own
    // Linkifier rather than trusting whatever hover last wrote; see
    // `primeLinkHover` for the gaps that closes and for the two dispatch
    // properties that keep a 1003 any-motion app from seeing motion reports.
    primeLinkHover(linkifierElement(term().element), e.clientX, e.clientY)
    const uri = hoveredLinkUri
    const decision = linkPressAction(
      {
        button: e.button,
        detail: e.detail,
        ctrlKey: e.ctrlKey,
        metaKey: e.metaKey,
        shiftKey: e.shiftKey,
        altKey: e.altKey,
      },
      {
        hoveredUri: uri,
        mouseTracking: true,
        hyperlinks: deps.hyperlinks(),
        isMac,
      },
    )
    if (!decision.suppress || uri === null) return
    e.stopPropagation()
    // xterm's element `mousedown` opens with `preventDefault(); this.focus()`
    // for EVERY press, before it even asks whether the app is tracking the
    // mouse. Neither runs for a press dux swallows, so both are done here:
    // the default action a terminal suppresses is the browser starting its
    // own text-selection drag over the rows. Note that xterm's SELECTION
    // service is disabled while the app captures the mouse, so there is no
    // local selection clear to restore alongside them (MEASURED in the
    // installed bundle: `SelectionService.handleMouseDown` returns early
    // when disabled unless the force-selection modifier is held).
    e.preventDefault()
    inFlight = { uri, x: e.clientX, y: e.clientY, open: decision.open }
    armOutsideRelease()
    // xterm's `mousedown` handler is what focuses the terminal, and it never
    // runs for a swallowed press, so do its job explicitly: a click into the
    // pane must still leave the keyboard pointed at the terminal.
    term().focus()
    // Only when something is actually about to OPEN. The sentence says dux
    // opened the link in your browser, and a press dux swallows without
    // opening (the preference turned off under a link already on screen) would
    // make that a lie. The hint teaches the hatch, and the hatch only matters
    // where opens happen.
    if (decision.open) raiseLinkForwardHint(isMac)
  }
  const onLinkReleaseCapture = (e: MouseEvent) => {
    if (isDuxReplay(e)) return
    // Only the PRIMARY release closes a swallowed press. Without this gate a
    // right release chorded on top of one consumed the record and got stopped
    // itself, which both unbalanced the right button's own report pair and
    // left the real left release to leak through as a release with no press.
    if (e.button !== 0) return
    const press = inFlight
    if (!press) return
    inFlight = null
    disarmOutsideRelease()
    // Paired with its press, always: a release forwarded on its own would be
    // a report for a gesture the app never saw begin.
    e.stopPropagation()
    const withinDragThreshold =
      Math.hypot(e.clientX - press.x, e.clientY - press.y) < DRAG_THRESHOLD_PX
    // Only a gesture that TRAVELLED needs a second resolution, which keeps the
    // extra hover dispatch off the ordinary click.
    let releaseUri: string | null = press.uri
    if (!withinDragThreshold) {
      primeLinkHover(linkifierElement(term().element), e.clientX, e.clientY)
      releaseUri = hoveredLinkUri
    }
    if (
      !linkReleaseOpens({
        open: press.open,
        withinDragThreshold,
        releaseUri,
        pressedUri: press.uri,
      })
    ) {
      return
    }
    openLink(
      {
        button: e.button,
        detail: e.detail,
        ctrlKey: e.ctrlKey,
        metaKey: e.metaKey,
        shiftKey: e.shiftKey,
        altKey: e.altKey,
      },
      press.uri,
    )
  }
  return {
    linkHandler: {
      activate: (event, uri) => {
        openLink(event, uri)
      },
      hover: (_event, uri) => {
        hoveredLinkUri = uri
      },
      leave: () => {
        hoveredLinkUri = null
      },
    },
    setTerminal: (t) => {
      terminal = t
    },
    attach: (el) => {
      container = el
      el.addEventListener("mousedown", onLinkPressCapture as EventListener, true)
      el.addEventListener("mouseup", onLinkReleaseCapture as EventListener, true)
    },
    activations: () => linkActivations,
    dispose: () => {
      container?.removeEventListener(
        "mousedown",
        onLinkPressCapture as EventListener,
        true,
      )
      container?.removeEventListener(
        "mouseup",
        onLinkReleaseCapture as EventListener,
        true,
      )
      container = null
      disarmOutsideRelease()
    },
  }
}
