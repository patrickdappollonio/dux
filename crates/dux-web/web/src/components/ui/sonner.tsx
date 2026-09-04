import { Toaster as Sonner, type ToasterProps } from "sonner"

import { useIsMobile } from "@/hooks/use-mobile"
import { CircleCheckIcon, InfoIcon, TriangleAlertIcon, OctagonXIcon, Loader2Icon } from "lucide-react"

// Which drags dismiss a toast.
//
// sonner infers its default directions by SPLITTING the position string, so
// "bottom-center" yields ["bottom", "center"]; "center" is not a direction, so
// a sideways swipe moves nothing and dismisses nothing. Naming the directions
// is what turns left/right back on. Down is kept because it is the gesture a
// bottom-anchored toast invites (push it back off the bottom edge), and "top"
// is deliberately absent: dragging a bottom toast upward is pulling it further
// INTO the screen, which should not throw it away.
//
// This is the touch story too. sonner drives the gesture from pointer events
// with `touch-action: none` on the toast, so a finger drag works exactly like a
// mouse drag, and it is the only dismissal a busy/loading toast has at all
// (sonner renders no close button for that type). The tiny X remains for mouse
// users; the swipe is what makes a toast dismissible on a phone or tablet.
export const TOAST_SWIPE_DIRECTIONS: ToasterProps["swipeDirections"] = [
  "bottom",
  "left",
  "right",
]

/// The same rule read off the top edge, for the phone's placement below.
///
/// The vertical direction follows the anchor rather than the other way round:
/// pushing a top toast back off the top edge is the gesture it invites, and
/// dragging it DOWN would pull it further into the screen, over the terminal.
export const TOAST_SWIPE_DIRECTIONS_TOP: ToasterProps["swipeDirections"] = [
  "top",
  "left",
  "right",
]

/// Where the toasts sit, per shell.
///
/// On a phone the bottom of every pane screen is the typing surface: the
/// compose bar and the terminal key rows live there, and a stack of toasts over
/// them covers the thing the user is answering with. So the phone anchors at
/// the top, ALWAYS, rather than only while a PTY is on screen: one home per
/// shell, and the bottom is spoken for on that shell generally. The computer
/// keeps the bottom, where nothing is competing for the corner.
///
/// The two offsets are the same expression on purpose. sonner switches from
/// `--offset-*` to `--mobile-offset-*` at its OWN 600px media query, which is
/// not the app's 768px shell breakpoint, so a 700px-wide phone in landscape
/// would otherwise read a different inset from the one this placement chose.
/// Only the named side is given; sonner fills the other three with its own
/// defaults (24px desktop, 16px mobile).
///
/// The safe-area inset is ours to add. The toaster is `position: fixed` and
/// sits outside the mobile root that pads for the notch, so without this a top
/// toast would land under the status bar on a notched phone (`viewport-fit=cover`
/// is set on the viewport meta, so the inset is real there and zero elsewhere).
const TOP_INSET = "calc(env(safe-area-inset-top) + 1rem)"
const BOTTOM_INSET = "calc(env(safe-area-inset-bottom) + 2.5rem)"

export function toastPlacement(isMobile: boolean): {
  position: NonNullable<ToasterProps["position"]>
  offset: NonNullable<ToasterProps["offset"]>
  mobileOffset?: ToasterProps["mobileOffset"]
  swipeDirections: ToasterProps["swipeDirections"]
} {
  if (isMobile) {
    return {
      position: "top-center",
      offset: { top: TOP_INSET },
      mobileOffset: { top: TOP_INSET },
      swipeDirections: TOAST_SWIPE_DIRECTIONS_TOP,
    }
  }
  // No `mobileOffset` on this branch, deliberately: the computer's placement is
  // left exactly as it was, and sonner's mobile variables are unreachable from
  // a viewport wide enough to be on this shell anyway.
  return {
    position: "bottom-center",
    offset: { bottom: BOTTOM_INSET },
    swipeDirections: TOAST_SWIPE_DIRECTIONS,
  }
}

// Per-tone icon color, so "this is fine" and "this is on fire" are not the same
// picture. Shape still differs per tone (check / info / triangle / octagon), so
// color is an addition to the signal and never the only carrier of it.
//
// `text-destructive` is the app's semantic error token. Success, warning and
// info follow the palette the rest of the web UI already uses for state
// (green for good, amber for caution, sky for informational), and the loading
// spinner stays muted because "in progress" is not a severity.
const TONE_ICON = {
  success: "size-4 text-green-500",
  info: "size-4 text-sky-400",
  warning: "size-4 text-amber-500",
  error: "size-4 text-destructive",
  loading: "size-4 animate-spin text-muted-foreground",
} as const

/// How many toasts stack before the rest queue behind them.
///
/// sonner's default is 3, which is low for a surface that now carries every
/// engine status: a multi-step operation can easily have three keyed statuses
/// open while an unrelated error arrives, and the fourth silently waits behind
/// them. Five fits comfortably on a desktop window.
export const VISIBLE_TOASTS_DESKTOP = 5

/// Phones keep sonner's 3. Vertical space is scarce there and the toasts sit
/// over the terminal, which is the thing the user is reading.
export const VISIBLE_TOASTS_MOBILE = 3

const Toaster = ({ ...props }: ToasterProps) => {
  const isMobile = useIsMobile()
  // Read live, so a rotation across the shell breakpoint moves the stack with
  // the shell rather than leaving it over the compose bar until the next toast.
  // Crossing it re-keys sonner's per-position list, so the toasts already up are
  // remounted: they survive (the store outlives the component) and their
  // dismissal timers restart. A busy toast is unaffected, having no timer of its
  // own, and a rotation mid-toast is rare enough to pay one restarted window for.
  const placement = toastPlacement(isMobile)
  return (
    <Sonner
      theme="dark"
      visibleToasts={isMobile ? VISIBLE_TOASTS_MOBILE : VISIBLE_TOASTS_DESKTOP}
      className="toaster group"
      {...placement}
      // Every toast now auto-dismisses on a severity-graded timer (see
      // `lib/notify.ts`), so the close button is a shortcut rather than the
      // only exit. Keep it for mouse users; touch users swipe.
      closeButton
      icons={{
        success: (
          <CircleCheckIcon className={TONE_ICON.success} />
        ),
        info: (
          <InfoIcon className={TONE_ICON.info} />
        ),
        warning: (
          <TriangleAlertIcon className={TONE_ICON.warning} />
        ),
        error: (
          <OctagonXIcon className={TONE_ICON.error} />
        ),
        loading: (
          <Loader2Icon className={TONE_ICON.loading} />
        ),
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      toastOptions={{
        classNames: {
          toast: "cn-toast",
        },
      }}
      {...props}
    />
  )
}

export { Toaster }
