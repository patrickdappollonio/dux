import * as React from "react"

import {
  currentShellLayout,
  currentLayoutChoice,
  isNarrowComputerLayout,
  isPhoneDevice,
  subscribeShellLayout,
  type ShellLayout,
} from "@/lib/shellLayout"

export { MOBILE_BREAKPOINT } from "@/lib/shellLayout"

/**
 * Whether this device shows the phone layout, for a caller that is not a
 * component. It reads the same answer as the hook (lib/shellLayout.ts), so
 * the two can never disagree. Outside a browser it is the desktop one.
 */
export function isMobileViewport(): boolean {
  return currentShellLayout() === "phone"
}

// `useSyncExternalStore` rather than state mirrored in an effect: the live value
// is read during render (no initial `undefined` flash, no synchronous
// `setState`) and the `false` server snapshot keeps it SSR-safe.
export function useIsMobile() {
  return React.useSyncExternalStore(subscribeShellLayout, isMobileViewport, () => false)
}

/** The person's layout choice on this device, live; null while automatic. */
export function useLayoutChoice(): ShellLayout | null {
  return React.useSyncExternalStore(subscribeShellLayout, currentLayoutChoice, () => null)
}

/** Whether this device is a phone whatever layout is chosen, live. */
export function useIsPhoneDevice(): boolean {
  return React.useSyncExternalStore(subscribeShellLayout, isPhoneDevice, () => false)
}

/** Whether the computer layout is shown narrower than the breakpoint, live. */
export function useIsNarrowComputerLayout(): boolean {
  return React.useSyncExternalStore(subscribeShellLayout, isNarrowComputerLayout, () => false)
}
