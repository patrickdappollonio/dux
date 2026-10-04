// Which auth banners this page load has been told to hide. Page memory only:
// "Dismiss" writes nothing, so the banner is back on the next load (see
// `components/AuthBanners.tsx`).

import { useSyncExternalStore } from "react"

export type AuthBannerId = "no_auth" | "weak"

// An immutable snapshot replaced on every change, so `useSyncExternalStore`
// sees a new identity exactly when something moved.
let dismissed: ReadonlySet<AuthBannerId> = new Set()
const listeners = new Set<() => void>()

export function dismissBanner(id: AuthBannerId): void {
  if (dismissed.has(id)) return
  dismissed = new Set([...dismissed, id])
  for (const l of [...listeners]) l()
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => listeners.delete(l)
}

function snapshot(): ReadonlySet<AuthBannerId> {
  return dismissed
}

export function useDismissedBanners(): ReadonlySet<AuthBannerId> {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}

/// Test seam: forget every dismissal.
export function resetBannerDismissals(): void {
  dismissed = new Set()
  for (const l of [...listeners]) l()
}
