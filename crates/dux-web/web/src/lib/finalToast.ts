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
// Raising a busy toast lives next door in `busyToast.ts`, because a busy toast
// is a different animal: sonner refuses to retire one on its own, so it needs a
// leak guard, and the user's auto-clear window does not apply to a state that is
// not final. What DOES belong here is retiring that guard. A busy toast is
// normally replaced in place by its final on the same id, so this raiser
// disarms whatever was armed for that id: otherwise a guard could fire later and
// dismiss the FINAL it was never armed for. That also means a caller can raise a
// busy and then a final on one id without knowing the guard exists, which is how
// the file-drop report works.

import { toast } from "sonner"

import { cancelBusyToastGuard } from "./busyToast"
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
  // This toast supersedes anything on the id, including a spinner whose guard is
  // still pending.
  cancelBusyToastGuard(opts.id)
  const options = {
    id: opts.id,
    duration: statusToastDuration(tone, opts.statusClearSeconds),
  }
  if (tone === "error") toast.error(message, options)
  else if (tone === "warning") toast.warning(message, options)
  else toast.success(message, options) // info / success
}
