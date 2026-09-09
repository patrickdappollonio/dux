import { Toaster as Sonner, type ToasterProps } from "sonner"

import { useIsMobile } from "@/hooks/use-mobile"
import { CircleCheckIcon, InfoIcon, TriangleAlertIcon, OctagonXIcon, Loader2Icon } from "lucide-react"

// Which drags dismiss a toast. Named explicitly because sonner infers its defaults
// by splitting the position string, and "center" is not a direction, so a sideways
// swipe would do nothing. Only the direction that pushes a toast off its own edge
// is listed. The swipe is the only dismissal a busy toast has, since sonner draws
// it no close button.
export const TOAST_SWIPE_DIRECTIONS: ToasterProps["swipeDirections"] = [
  "bottom",
  "left",
  "right",
]

/// The same rule read off the top edge, for the phone's placement below: the
/// vertical direction follows the anchor, so a top toast is pushed up and off.
export const TOAST_SWIPE_DIRECTIONS_TOP: ToasterProps["swipeDirections"] = [
  "top",
  "left",
  "right",
]

/// Where the toasts sit, per shell: the phone anchors at the TOP always, because
/// the bottom of a pane screen is its typing surface, and the computer keeps the
/// bottom. Both offsets carry the same expression, because sonner switches to its
/// mobile variables at its own 600px query rather than the app's 768px breakpoint,
/// and the safe-area inset is added here, since the toaster is fixed and sits
/// outside the mobile root that pads for the notch.
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
  // No `mobileOffset` on this branch: sonner's mobile variables are unreachable
  // from a viewport wide enough to be on this shell.
  return {
    position: "bottom-center",
    offset: { bottom: BOTTOM_INSET },
    swipeDirections: TOAST_SWIPE_DIRECTIONS,
  }
}

// Per-tone icon color, added to a shape that already differs per tone, so color is
// never the only carrier. The loading spinner stays muted: in progress is not a
// severity.
const TONE_ICON = {
  success: "size-4 text-green-500",
  info: "size-4 text-sky-400",
  warning: "size-4 text-amber-500",
  error: "size-4 text-destructive",
  loading: "size-4 animate-spin text-muted-foreground",
} as const

/// How many toasts stack before the rest queue behind them. Above sonner's default
/// of 3, which a multi-step operation's keyed statuses fill on their own.
export const VISIBLE_TOASTS_DESKTOP = 5

/// Phones keep sonner's 3. Vertical space is scarce there and the toasts sit
/// over the terminal, which is the thing the user is reading.
export const VISIBLE_TOASTS_MOBILE = 3

const Toaster = ({ ...props }: ToasterProps) => {
  const isMobile = useIsMobile()
  // Read live, so a rotation across the shell breakpoint moves the stack with the
  // shell. Crossing it re-keys sonner's per-position list, so open toasts remount
  // and their dismissal timers restart, which is the accepted cost.
  const placement = toastPlacement(isMobile)
  return (
    <Sonner
      theme="dark"
      visibleToasts={isMobile ? VISIBLE_TOASTS_MOBILE : VISIBLE_TOASTS_DESKTOP}
      className="toaster group"
      {...placement}
      // Every toast auto-dismisses on a severity-graded timer (`lib/notify.ts`), so
      // the close button is a mouse shortcut rather than the only exit.
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
