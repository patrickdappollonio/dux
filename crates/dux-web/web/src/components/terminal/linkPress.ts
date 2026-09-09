// One click on a hyperlink opens one tab, on the clicker's side. This machine is
// the only thing in the pane that opens a terminal hyperlink, and it holds the
// three pieces that decision needs: the hover cache (the OSC 8 uri under the
// pointer, as xterm's own Linkifier resolves it), the in-flight press record,
// and the activation counter.
//
// Its readers are xterm's own Linkifier through the `linkHandler` this exposes
// (the tracking-off path), the capture-phase release below (the tracking-on
// path), and the touch tap probe, which replays a synthetic hover-and-click and
// compares `activations()` across it to learn whether a tap hit a link.
//
// It abstains entirely under the force-local-selection modifier: the press
// passes through, xterm forwards nothing under it, and dux opens nothing.
// Swallowing instead would make a link the one place the selection hatch fails;
// and since the press passes through, xterm's Linkifier still activates on the
// drag-end mouseup, so the activate side refuses it too or a selection opens a
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
  /// Hand this to the `Terminal` constructor. `hover` and `leave` exist for the
  /// capture intercept: they are the only public read of whether a link is at a
  /// point, since the OSC 8 uri lives in an internal service.
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
  // Written by the handler's `hover` and `leave`, and driven at press time by
  // `primeLinkHover`: a passive mousemove is not enough, because the buffer can
  // scroll under a still pointer and a page's first click may follow no move.
  let hoveredLinkUri: string | null = null
  let terminal: Terminal | null = null
  let container: Element | null = null
  const term = (): Terminal => {
    if (terminal === null) throw new Error("link machine used before its terminal")
    return terminal
  }

  // The one place a terminal hyperlink is opened, reached from xterm's Linkifier
  // `activate` and from the capture intercept's release. Shared so both get the
  // same `linkActivateAction` truth table, the same `noopener,noreferrer`
  // window, and the same activation counter the touch probe reads.
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

  // While the app in the PTY tracks the mouse, a click on an OSC 8 link would
  // open the page twice: dux's `window.open` here, plus the forwarded mouse
  // report, which an agent CLI answers by opening the url on the server's
  // machine. Only the first reaches the clicker, so dux is the sole opener and
  // the click that dispatched a link is withheld from the app entirely. This
  // diverges from iTerm2, Ghostty and kitty, which give the click to the app and
  // reserve links for a modifier; the hatch in `linkHatchHeld` forwards the
  // click and refuses dux's open.
  //
  // Mechanism, measured against the installed xterm: mouse reports come from DOM
  // mouse listeners only, through `bindMouse` in `CoreBrowserTerminal`, which
  // registers the document-level release and drag reporters inside its element
  // `mousedown` handler. Swallowing the press therefore suppresses the whole
  // report pair, a release outside the pane included.
  //
  // Every part of the shape is load-bearing:
  //  - Capture phase on the container, so this decides before xterm's listeners
  //    on descendants; `stopPropagation`, never `stopImmediatePropagation`,
  //    which would silence dux's own bubble-phase copy-on-select.
  //  - Press time, because xterm emits the press report from `mousedown`, and
  //    press-activated TUI controls act on a lone press.
  //  - Primary button only, so right-click paste and context menus are untouched.
  //  - Tracking on only: with nothing capturing the mouse there is no report to
  //    suppress, and swallowing would cost the focus grab, the selection clear
  //    and the drag-select that starts on a link.
  //  - dux's own replays are tagged and skipped (`lib/termreplay.ts`); an
  //    `isTrusted` check would skip every test and assistive-technology click.
  let inFlight: { uri: string; x: number; y: number; open: boolean } | null = null
  let outsideReleaseWatch: ((e: MouseEvent) => void) | null = null
  const disarmOutsideRelease = () => {
    if (!outsideReleaseWatch) return
    document.removeEventListener("mouseup", outsideReleaseWatch, true)
    outsideReleaseWatch = null
  }
  // A swallowed press may be released anywhere, so the in-flight record has to
  // clear either way or the next click reads a stale one. This observes and
  // clears and never stops propagation: xterm's document reporter was never
  // attached for a swallowed press, and a swallowing one-shot would eat an
  // unrelated mouseup after a release the window never saw.
  const armOutsideRelease = () => {
    disarmOutsideRelease()
    const watch = (e: MouseEvent) => {
      // The PRIMARY release ends the gesture; a chorded right release while
      // the left button is still down is somebody else's event.
      if (e.button !== 0) return
      disarmOutsideRelease()
      // A release inside the pane belongs to the capture handler below, which
      // runs after this one, so the record must be left alone for it.
      if (e.target instanceof Node && container?.contains(e.target)) return
      inFlight = null
    }
    outsideReleaseWatch = watch
    document.addEventListener("mouseup", watch, true)
  }
  const onLinkPressCapture = (e: MouseEvent) => {
    if (isDuxReplay(e)) return
    // Non-primary buttons must not touch the in-flight record, hence this gate
    // before the reset below: a right press chorded onto a live left one would
    // wipe a swallowed press, leaking its release as a report for a gesture the
    // app never saw begin.
    if (e.button !== 0) return
    // A new PRIMARY press ends the previous gesture whatever happened to it,
    // so a release the window never delivered cannot wedge the next click.
    inFlight = null
    disarmOutsideRelease()
    if (term().modes.mouseTrackingMode === "none") return
    // Resolved synchronously through xterm's own Linkifier rather than from
    // whatever hover last wrote; `primeLinkHover` has the gaps that closes.
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
    // for every press, and neither runs for one dux swallows: the default being
    // suppressed is the browser's own text-selection drag over the rows. There
    // is no local selection clear to restore alongside them, because xterm's
    // selection service is disabled while the app captures the mouse.
    e.preventDefault()
    inFlight = { uri, x: e.clientX, y: e.clientY, open: decision.open }
    armOutsideRelease()
    // xterm's `mousedown` handler is what focuses the terminal and never runs
    // for a swallowed press, so a click into the pane must refocus here.
    term().focus()
    // Only where something is about to open: the sentence says dux opened the
    // link in your browser, and a swallowed press that opens nothing would make
    // it a lie. The hint teaches the hatch, which matters only where opens are.
    if (decision.open) raiseLinkForwardHint(isMac)
  }
  const onLinkReleaseCapture = (e: MouseEvent) => {
    if (isDuxReplay(e)) return
    // Only the primary release closes a swallowed press: a right release chorded
    // on top of one would consume the record and be stopped itself, unbalancing
    // the right button's report pair and leaking the real left release.
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
