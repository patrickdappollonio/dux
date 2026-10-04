// How tall a shell is. The auth banners (`components/AuthBanners.tsx`) sit in
// the page's flow above the shell and publish their height as
// `--dux-app-top`; every shell subtracts it, so a banner pushes the app down
// rather than covering its header, its cog or the phone's flap. With no banner
// the variable is unset and the shells are exactly as tall as before.

const APP_TOP = "var(--dux-app-top, 0px)"

/// The height of a shell sized to the viewport, or to the measured visual
/// viewport while the soft keyboard is up.
export function shellHeight(viewportHeight: number | null): string {
  return viewportHeight === null
    ? `calc(100svh - ${APP_TOP})`
    : `calc(${viewportHeight}px - ${APP_TOP})`
}

/// The same, as a class, for the desktop shell. Spelled out in full so
/// Tailwind sees it.
export const SHELL_HEIGHT_CLASS = "h-[calc(100svh-var(--dux-app-top,0px))]"

/// The phone shells' top inset. A banner stack clears the notch itself, and
/// sets `--dux-app-safe-top` to zero while it shows so the inset is not paid
/// twice.
export const SHELL_SAFE_TOP = "var(--dux-app-safe-top, env(safe-area-inset-top))"
