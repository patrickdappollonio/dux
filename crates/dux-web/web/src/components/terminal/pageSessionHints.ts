// THE TWO ONCE-PER-PAGE-SESSION HINT LATCHES.
//
// Both hints teach a modifier, and both are shown at most ONCE PER PAGE LOAD,
// not once per pane. The distinction is the whole reason these are module
// scope: the pane remounts on every agent switch, every tab switch and every
// rotation past the breakpoint, so a component ref would re-arm the hint over
// and over for a user who has already read it and learned the chord.
//
// It also has to be once per page rather than once per pane for a second
// reason: two panes would otherwise mean two toasts, and with no shared id
// there is nothing to merge them into one.
//
// These are page-lifetime state and are therefore on the deliberate-registry
// roster, alongside the cross-module registries in `lib/`. They are here, in
// one file, so that roster is a list rather than a claim.
import { notifyInfo } from "@/lib/notify"

let mouseCaptureHintFired = false
let linkForwardHintFired = false

/// Has the mouse-capture hint already fired this page session? Read by
/// `copyOnSelectAction`, which decides between copying and hinting.
export function mouseCaptureHintShown(): boolean {
  return mouseCaptureHintFired
}

/// "The app is using the mouse; hold the modifier to select locally."
///
/// Raised on the FIRST drag that the app captured, never on a plain click.
/// It carries no toast id, deliberately: the latch above means there is never a
/// second raise for an id to deduplicate, and an id would only put the message
/// at risk of pinning itself open. It takes the configured display window like
/// every other toast, and it is toned INFO because a neutral instruction should
/// look like one.
export function raiseMouseCaptureHint(isMac: boolean): void {
  if (mouseCaptureHintFired) return
  mouseCaptureHintFired = true
  notifyInfo(
    `This app is using the mouse. Hold ${
      isMac ? "⌥ Option" : "Shift"
    } and drag to select and copy to your device.`,
  )
}

/// "dux opened that link here; hold the chord to send the click to the app."
///
/// Raised only when an open ACTUALLY HAPPENS. A press dux swallows without
/// opening (the hyperlinks preference switched off under a link already on
/// screen, a refused scheme) would make the sentence a lie, and the hatch it
/// teaches only matters where opens happen.
export function raiseLinkForwardHint(isMac: boolean): void {
  if (linkForwardHintFired) return
  linkForwardHintFired = true
  notifyInfo(
    `dux opened that link in your browser. Hold ${
      isMac ? "⌘ Command" : "Ctrl"
    } and click to send the click to the app instead.`,
  )
}
