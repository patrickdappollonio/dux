// The once-per-page-session hint latches. Both hints teach a modifier and are
// shown at most once per page load, never once per pane, which is why they are
// module scope: the pane remounts on every agent switch, tab switch and rotation
// past the breakpoint, and two live panes would otherwise raise two toasts with
// no shared id to merge them.
//
// They are page-lifetime state, and they live in one file so the
// deliberate-registry roster is a list rather than a claim.
import { notifyInfo } from "@/lib/notify"

let mouseCaptureHintFired = false
let linkForwardHintFired = false

/// Has the mouse-capture hint already fired this page session? Read by
/// `copyOnSelectAction`, which decides between copying and hinting.
export function mouseCaptureHintShown(): boolean {
  return mouseCaptureHintFired
}

/// Raised on the first drag the app captured, never on a plain click. It carries
/// no toast id: the latch above means there is never a second raise to
/// deduplicate, and an id would only risk pinning the message open. Info-toned,
/// on the configured display window like every other toast.
export function raiseMouseCaptureHint(isMac: boolean): void {
  if (mouseCaptureHintFired) return
  mouseCaptureHintFired = true
  notifyInfo(
    `This app is using the mouse. Hold ${
      isMac ? "⌥ Option" : "Shift"
    } and drag to select and copy to your device.`,
  )
}

/// Raised only where an open actually happens: a press dux swallows without
/// opening would make the sentence a lie, and the hatch it teaches matters only
/// where opens happen.
export function raiseLinkForwardHint(isMac: boolean): void {
  if (linkForwardHintFired) return
  linkForwardHintFired = true
  notifyInfo(
    `dux opened that link in your browser. Hold ${
      isMac ? "⌘ Command" : "Ctrl"
    } and click to send the click to the app instead.`,
  )
}
