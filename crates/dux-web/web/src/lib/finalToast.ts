// The one way to raise a FINAL (non-busy) toast.
//
// This exists so a client-originated toast cannot quietly ignore the user's
// configured dismiss window. `statusToastDuration` owns the policy and has been
// exported all along, but the function that actually raised a toast with it was
// private to the store, so anything else reaching for sonner directly got the
// library's own default duration instead. Rather than adding a second raiser
// alongside the store's, the store now routes its non-busy tones through this
// one too, so there is exactly one implementation to keep honest.
//
// Busy deliberately does NOT live here. A busy toast is a different animal: it
// is normally replaced in place by its keyed final, sonner refuses to retire it
// on its own, and the store therefore schedules and cancels its own leak guard
// against ids it is tracking. That bookkeeping belongs with the store's status
// map, not in a general-purpose raiser.

import { toast } from "sonner"

import { statusToastDuration } from "./statusToast"

/// Tones that are a FINAL state. `busy` is excluded on purpose (see above).
export type FinalTone = "info" | "success" | "warning" | "error"

/// Raise (or replace, when `id` repeats) a final toast, on the severity-graded
/// window derived from the user's `ui.status_clear_seconds`.
///
/// `statusClearSeconds` is that setting; `null`/`undefined` covers the window
/// before the bootstrap document lands, and a configured `0` keeps its
/// documented meaning of disabling auto-clear.
export function showFinalToast(
  tone: FinalTone,
  message: string,
  opts: { id: string; statusClearSeconds: number | null | undefined },
): void {
  if (!message) return
  const options = {
    id: opts.id,
    duration: statusToastDuration(tone, opts.statusClearSeconds),
  }
  if (tone === "error") toast.error(message, options)
  else if (tone === "warning") toast.warning(message, options)
  else toast.success(message, options) // info / success
}
