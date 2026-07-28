// How long an engine-status toast stays on screen, by tone.
//
// Every tone auto-dismisses. A toast the user has to hunt down and close is
// friction, and the previous policy (warning, error and busy all pinned at
// Infinity) meant the bottom of the screen slowly filled with stale statuses
// nobody had clicked away. Severity is carried by the icon and its color, not
// by making the user do the dismissing.
//
// Severity does buy TIME, though: an error that vanishes as fast as a success
// defeats the point of an error toast, so the windows are graded off the one
// user-configurable base (`ui.status_clear_seconds`, the same setting the
// engine uses to expire Info statuses) instead of being three hardcoded
// numbers. One knob still moves all of them.

/// Fallback window (seconds) used before the bootstrap document lands, matching
/// the config default for `ui.status_clear_seconds`.
export const DEFAULT_STATUS_CLEAR_SECONDS = 6

/// A warning stays up twice as long as a success.
export const WARNING_DURATION_FACTOR = 2

/// An error stays up four times as long as a success: it is the tone the user
/// most needs to actually read, and the one most likely to arrive while they
/// are looking somewhere else.
export const ERROR_DURATION_FACTOR = 4

/// Hard ceiling for a busy/loading toast.
///
/// A busy toast is normally replaced in place by its keyed final, so this is
/// not a readability window: it is a leak guard for the case where the final
/// never arrives (the events socket dropped mid-operation), which is exactly
/// the stranded-spinner the user reported. It must stay comfortably above
/// `dux_core::statusline::BUSY_TIMEOUT` (20s), after which the engine itself
/// upgrades a stranded keyed Busy to a Warning and pushes that replacement, so
/// the guard can never fire while an operation is still live and make the work
/// look like it stopped.
export const BUSY_TOAST_MAX_MS = 60_000

/// Resolve the sonner `duration` for a status toast.
///
/// `statusClearSeconds` is the server's `ui.status_clear_seconds`; `null` /
/// `undefined` covers the pre-bootstrap window. A configured `0` keeps its
/// documented meaning of "disable auto-clear", and it applies to FINAL states
/// only: busy is not a final, so it always keeps its leak guard.
export function statusToastDuration(
  tone: string,
  statusClearSeconds: number | null | undefined,
): number {
  if (tone === "busy") return BUSY_TOAST_MAX_MS

  const secs = statusClearSeconds ?? DEFAULT_STATUS_CLEAR_SECONDS
  if (secs <= 0) return Infinity

  const base = secs * 1000
  if (tone === "error") return base * ERROR_DURATION_FACTOR
  if (tone === "warning") return base * WARNING_DURATION_FACTOR
  return base // info / success, and any tone the server adds later
}
