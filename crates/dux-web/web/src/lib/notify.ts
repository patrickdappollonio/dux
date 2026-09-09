// The one place in the web UI that raises a notification: sonner's `toast` may be
// imported here and by `components/ui/sonner.tsx` only, an exact set asserted by
// `notifyBoundary.test.ts`. sonner is the countdown mechanism; dux owns the policy.
//
// Every tone auto-dismisses on a window graded off the one configurable base,
// except `sticky`, which waits for the user, and a busy spinner, which sonner
// never auto-closes at all and which this module's leak guard retires by
// replacing it with a warning rather than taking it off the screen.

import { toast } from "sonner"

/// Fallback window (seconds) used before the bootstrap document lands, matching
/// the config default for `ui.status_clear_seconds`.
export const DEFAULT_STATUS_CLEAR_SECONDS = 6

/// A warning stays up three times as long as a success. Must stay equal to
/// `WARNING_CLEAR_FACTOR` in `crates/dux-core/src/statusline.rs`.
export const WARNING_DURATION_FACTOR = 3

/// An error stays up four times as long as a success: it is the tone most likely
/// to arrive while the user is looking somewhere else.
export const ERROR_DURATION_FACTOR = 4

/// Hard ceiling for a busy/loading toast: a leak guard for when no further word
/// arrives at all, not a readability window.
///
/// Must stay comfortably above `dux_core::statusline::BUSY_TIMEOUT`, the cadence
/// a live server answers on, so the guard firing means the server went quiet.
export const BUSY_TOAST_MAX_MS = 60_000

/// Tones that are a final state. `busy` is excluded on purpose: the user's
/// auto-clear window and its `0` opt-out do not apply to it.
export type FinalTone = "info" | "success" | "warning" | "error"

export interface NotifyOptions {
  /// Raise on a fixed id: an id means replacement, not de-duplication, and a
  /// repeated raise on one restarts the countdown and can pin the toast open.
  id?: string
  /// Wait for the user instead of for a clock (`duration: Infinity`). Reserved for
  /// a notification the user must act on outside the toast, or where something may
  /// have been lost or left half-done.
  sticky?: boolean
}

// The user's configured window, at module scope rather than threaded through call
// sites: read where the raise happens, it cannot be captured stale by a closure.
let configuredStatusClearSeconds: number | null | undefined = undefined

/// Publish the user's `ui.status_clear_seconds`. Called when the bootstrap
/// document lands (and again whenever it is refetched after a config change).
export function setStatusClearSeconds(secs: number | null | undefined): void {
  configuredStatusClearSeconds = secs
}

/// Resolve the sonner `duration` for a notification of `tone`. `null`/`undefined`
/// `statusClearSeconds` is the pre-bootstrap window; a configured `0` disables
/// auto-clear for final tones only, since busy always keeps its leak guard.
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

const busyGuards = new Map<string, ReturnType<typeof setTimeout>>()

// Disarm the guard armed for `id`. Every path that changes what sits on an id
// goes through here, so a guard can only ever retire the spinner it was armed for.
function cancelBusyGuard(id: string): void {
  const handle = busyGuards.get(id)
  if (handle === undefined) return
  clearTimeout(handle)
  busyGuards.delete(id)
}

// The one call into sonner for a final tone. Everything above resolves to this.
function raiseFinal(tone: string, message: string, duration: number, id?: string): void {
  const options = id === undefined ? { duration } : { id, duration }
  if (tone === "error") toast.error(message, options)
  else if (tone === "warning") toast.warning(message, options)
  else if (tone === "info") toast.info(message, options)
  else toast.success(message, options)
}

/// Raise a final (non-busy) notification. The window comes from the user's
/// configured `ui.status_clear_seconds`, graded by tone, unless `sticky` is set.
export function notify(tone: FinalTone, message: string, opts: NotifyOptions = {}): void {
  if (!message) return
  // This raise supersedes anything on the id, including a spinner whose guard is
  // still pending.
  if (opts.id !== undefined) cancelBusyGuard(opts.id)
  const duration = opts.sticky
    ? Infinity
    : statusToastDuration(tone, configuredStatusClearSeconds)
  raiseFinal(tone, message, duration, opts.id)
}

/// A neutral, informational notification.
export function notifyInfo(message: string, opts: NotifyOptions = {}): void {
  notify("info", message, opts)
}

/// Something the user asked for finished, and finished well.
export function notifySuccess(message: string, opts: NotifyOptions = {}): void {
  notify("success", message, opts)
}

/// Something is off but the operation still landed, or it can be retried freely.
export function notifyWarning(message: string, opts: NotifyOptions = {}): void {
  notify("warning", message, opts)
}

/// Something failed.
export function notifyError(message: string, opts: NotifyOptions = {}): void {
  notify("error", message, opts)
}

/// Where a spinner's eventual final is expected to come from, which decides what
/// the leak guard says when it never arrives: `wire` silence points at the log,
/// `local` silence means a request this tab made is still in flight.
export type BusyOrigin = "wire" | "local"

/// What a spinner is replaced with when its final never arrives. Neither wording
/// may claim the operation ended, because nothing here knows that.
export function strandedBusyMessage(
  message: string,
  origin: BusyOrigin = "wire",
): string {
  const seconds = Math.round(BUSY_TOAST_MAX_MS / 1000)
  if (origin === "local") {
    return `Still waiting on the server for "${message}" after ${seconds} seconds. The request has not been answered yet; nothing has been lost, and the outcome will replace this as soon as it arrives.`
  }
  return `No word from dux about "${message}" for ${seconds} seconds. The operation may still be running, and the connection may simply have dropped. Check dux.log if it never reports back.`
}

/// Raise (or replace, when `id` repeats) a busy spinner, armed with its leak
/// guard. `id` is required: both the replacement and the guard need a name to aim
/// at. `origin` decides only what the guard says; see [`BusyOrigin`].
///
/// The guard's warning is deliberately not sticky: the real final still replaces
/// it whenever it turns up, and one pinned toast per stranded spinner buries the screen.
export function notifyBusy(
  message: string,
  opts: { id: string; origin?: BusyOrigin },
): void {
  if (!message) return
  const duration = statusToastDuration("busy", null)
  const origin = opts.origin ?? "wire"
  // Whatever was armed for this id is now stale: this call replaces the toast.
  cancelBusyGuard(opts.id)
  busyGuards.set(
    opts.id,
    setTimeout(() => {
      busyGuards.delete(opts.id)
      notify("warning", strandedBusyMessage(message, origin), { id: opts.id })
    }, duration),
  )
  toast.loading(message, { id: opts.id, duration })
}

/// Raise a notification whose tone arrived over the wire as a string, correlated
/// by id so a busy and its final replace each other in place. The tone is
/// deliberately not narrowed: an unrecognised tone still renders, neutrally.
///
/// A wire `info` shows the success icon: the engine's status line has no
/// `success`, so `Info` is the tone its finished operations report in.
export function notifyStatus(
  tone: string,
  message: string,
  opts: { id: string; sticky?: boolean },
): void {
  if (!message) return
  if (tone === "busy") {
    notifyBusy(message, { id: opts.id })
    return
  }
  notify(tone === "info" ? "success" : (tone as FinalTone), message, opts)
}

/// Take a notification off the screen and disarm any guard armed for it.
export function dismissNotification(id: string): void {
  cancelBusyGuard(id)
  toast.dismiss(id)
}
