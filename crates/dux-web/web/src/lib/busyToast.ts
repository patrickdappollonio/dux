// The one way to raise a BUSY (spinner) toast, and the leak guard that retires
// it.
//
// This is the busy counterpart to `finalToast.ts`, and it lives beside it for
// the same reason: there must be exactly ONE implementation of the policy, or a
// client-originated spinner and an engine-status spinner will drift apart.
//
// It was extracted from the store rather than written a second time. The store's
// comment explaining why a busy toast needs a guard at all is reproduced here
// because the guard came with it: sonner deliberately never auto-closes a
// `loading` toast. Its close-timer effect returns early on
// `toast.type === 'loading'`, so the duration passed to `toast.loading` is
// inert, it renders no close button for that type, and it ignores the swipe
// gesture (all three pinned by tests in `components/ui/sonner.test.tsx`, so a
// sonner upgrade that loosens any of them fails loudly). A busy toast therefore
// has no exit of its own, and if its final never arrives the spinner sits there
// forever claiming work is still happening. So the dismissal is scheduled here.
//
// The guard is always cancelled before a toast on that id changes, so it can
// only ever dismiss the exact spinner it was armed for: never a later final, and
// never a fresh busy that reused the key. `showFinalToast` cancels it too, which
// is what lets a caller raise a busy and then a final on one id without knowing
// this module exists.

import { toast } from "sonner"

import { statusToastDuration } from "./statusToast"

const busyToastGuards = new Map<string, ReturnType<typeof setTimeout>>()

/// Disarm the guard for `id`, because whatever it was armed for is being
/// replaced or dismissed by the caller.
export function cancelBusyToastGuard(id: string): void {
  const handle = busyToastGuards.get(id)
  if (handle === undefined) return
  clearTimeout(handle)
  busyToastGuards.delete(id)
}

/// Raise (or replace, when `id` repeats) a busy toast, armed with its leak
/// guard.
///
/// There is no `statusClearSeconds` parameter on purpose: a busy toast is not a
/// final state, so the user's auto-clear window (including its documented `0`
/// opt-out) does not apply to it. `statusToastDuration` answers
/// `BUSY_TOAST_MAX_MS` for this tone whatever it is handed, and that is a leak
/// guard rather than a readability window.
export function showBusyToast(message: string, opts: { id: string }): void {
  if (!message) return
  const duration = statusToastDuration("busy", null)
  // Whatever was armed for this id is now stale: this call replaces the toast.
  cancelBusyToastGuard(opts.id)
  busyToastGuards.set(
    opts.id,
    setTimeout(() => {
      busyToastGuards.delete(opts.id)
      toast.dismiss(opts.id)
    }, duration),
  )
  toast.loading(message, { id: opts.id, duration })
}

/// Take a toast off the screen and disarm any guard armed for it.
export function dismissBusyToast(id: string): void {
  cancelBusyToastGuard(id)
  toast.dismiss(id)
}
