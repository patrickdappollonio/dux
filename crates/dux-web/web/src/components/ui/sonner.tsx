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
  return (
    <Sonner
      theme="dark"
      visibleToasts={isMobile ? VISIBLE_TOASTS_MOBILE : VISIBLE_TOASTS_DESKTOP}
      className="toaster group"
      position="bottom-center"
      offset={{ bottom: "calc(env(safe-area-inset-bottom) + 2.5rem)" }}
      swipeDirections={TOAST_SWIPE_DIRECTIONS}
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
