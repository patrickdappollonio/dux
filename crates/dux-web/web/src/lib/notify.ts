// The ONE place in the web UI that raises a notification.
//
// This file is the only module under `src/` allowed to import sonner's `toast`
// (the Toaster component in `components/ui/sonner.tsx` is the other permitted
// sonner importer, and it imports the component, not the raiser). That boundary
// is asserted as an EXACT set by `notifyBoundary.test.ts` rather than left to
// good intentions, because good intentions were already tried: `finalToast.ts`
// was meant to be the single raiser and 91 call sites imported sonner directly
// anyway, taking the library's bare 4s default and ignoring the user's
// configured window entirely. A rule nothing checks is a rule that decays.
//
// sonner stays the COUNTDOWN MECHANISM and dux owns the POLICY. sonner already
// implements pause-on-hover, pause-while-the-tab-is-hidden, swipe-to-dismiss
// and stacking, all of which are wanted; reimplementing them would be a large
// surface bought for nothing. What dux decides is how long a notification lives
// and whether it waits for a clock or for the user.
//
// Three things live here:
//
//   1. The severity-graded windows. Every tone auto-dismisses. A toast the user
//      has to hunt down and close is friction, and the previous policy (warning,
//      error and busy all pinned at Infinity) meant the bottom of the screen
//      slowly filled with stale statuses nobody had clicked away. Severity does
//      buy TIME, though: an error that vanishes as fast as a success defeats the
//      point of an error, so the windows are graded off the one user-configurable
//      base (`ui.status_clear_seconds`) instead of being three hardcoded numbers.
//      One knob still moves all of them.
//
//   2. `sticky`, a boolean orthogonal to tone, for the notification the user must
//      ACT on outside the toast to recover from, or where something may have been
//      lost or left half-done. It gets `duration: Infinity` and waits. Sticky is
//      still ESCAPABLE: sonner's swipe gate is `disabled = toastType ===
//      'loading'` and its close button is gated the same way, neither of them on
//      the duration, so an Infinity toast keeps both exits (pinned by
//      `notifySticky.dom.test.tsx`).
//
//   3. The busy leak guard. sonner deliberately never auto-closes a `loading`
//      toast: its close-timer effect returns early on `toast.type === 'loading'`,
//      so the duration handed to `toast.loading` is inert, it renders no close
//      button for that type, and it ignores the swipe (all three pinned by tests
//      in `components/ui/sonner.test.tsx`, so a sonner upgrade that loosens any
//      of them fails loudly). A busy toast therefore has no exit of its own, and
//      if its final never arrives the spinner sits there forever claiming work is
//      still happening. So the retirement is scheduled here.
//
//      It retires the spinner by REPLACING it with a warning, never by taking it
//      off the screen. A spinner that simply disappears tells the user their
//      operation is over when nobody knows any such thing, and that is exactly
//      the report this guard's wording now answers: an agent creation whose
//      spinner vanished mid-clone and whose agent turned up minutes later.

import { toast } from "sonner"

/// Fallback window (seconds) used before the bootstrap document lands, matching
/// the config default for `ui.status_clear_seconds`.
export const DEFAULT_STATUS_CLEAR_SECONDS = 6

/// A warning stays up three times as long as a success.
///
/// Must stay equal to `WARNING_CLEAR_FACTOR` in
/// `crates/dux-core/src/statusline.rs`, which gives a warning the same lifetime
/// on the terminal UI's status line; a Rust test reads this file and fails if
/// the two drift.
export const WARNING_DURATION_FACTOR = 3

/// An error stays up four times as long as a success: it is the tone the user
/// most needs to actually read, and the one most likely to arrive while they
/// are looking somewhere else.
export const ERROR_DURATION_FACTOR = 4

/// Hard ceiling for a busy/loading toast.
///
/// A busy toast is normally replaced in place by its keyed final, so this is
/// not a readability window: it is a leak guard for the case where no further
/// word arrives at all (the events socket dropped mid-operation).
///
/// It must stay comfortably above `dux_core::statusline::BUSY_TIMEOUT` (20s),
/// which is the cadence the engine works to on a key it still holds an
/// operation for: at the timeout it either upgrades a stranded keyed Busy to a
/// Warning or, when the operation is genuinely still running, re-sends the busy
/// on the same key. Either way a live server refreshes this guard long before
/// it can fire, so the guard firing means the SERVER has gone quiet, never that
/// the operation finished.
export const BUSY_TOAST_MAX_MS = 60_000

/// Tones that are a FINAL state. `busy` is excluded on purpose: it is not final,
/// so the user's auto-clear window (including its documented `0` opt-out) does
/// not apply to it.
export type FinalTone = "info" | "success" | "warning" | "error"

export interface NotifyOptions {
  /// Raise on a fixed id.
  ///
  /// An id means REPLACEMENT, not de-duplication: a second raise on the same id
  /// takes over the toast that is already there. That is right for a Busy
  /// handing off to its final, and wrong for anything repeat-prone, because
  /// sonner resets a toast's remaining time only when its DURATION changes while
  /// re-running its close timer on every re-raise. A repeated raise on a fixed
  /// id therefore restarts the countdown and can pin the toast open forever.
  /// Omit the id and every raise is its own event on its own clock.
  id?: string
  /// Wait for the user instead of for a clock (`duration: Infinity`).
  ///
  /// Reserve it for a notification the user must act on OUTSIDE the toast to
  /// recover from, or where something may have been lost or left half-done. A
  /// path that exists nowhere else on screen is the archetype.
  sticky?: boolean
}

// The user's configured window, published here by whoever learns it (the store,
// when the bootstrap document lands) and read on the way past on every raise.
//
// It lives at module scope rather than being threaded through every call site
// for a specific reason, documented in CLAUDE.md's clipboard-paste tenet: a
// raise registered in a mount effect that read the setting out of its render
// closure pinned every toast from that pane to the pre-bootstrap default for the
// life of the pane. Components worked around it with refs. With the value living
// where the raise happens there is nothing left to capture, so there is nothing
// left to capture STALE.
let configuredStatusClearSeconds: number | null | undefined = undefined

/// Publish the user's `ui.status_clear_seconds`. Called when the bootstrap
/// document lands (and again whenever it is refetched after a config change).
export function setStatusClearSeconds(secs: number | null | undefined): void {
  configuredStatusClearSeconds = secs
}

/// The window every raise is currently measured against. `null`/`undefined`
/// means the bootstrap document has not landed yet.
export function currentStatusClearSeconds(): number | null | undefined {
  return configuredStatusClearSeconds
}

/// Resolve the sonner `duration` for a notification of `tone`.
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

const busyGuards = new Map<string, ReturnType<typeof setTimeout>>()

// Disarm the guard armed for `id`, because whatever it was armed for is being
// replaced or dismissed.
//
// Every path that changes what sits on an id goes through here first, which is
// what makes the guard able to dismiss only the exact spinner it was armed for:
// never a later final, and never a fresh busy that reused the key.
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

/// Raise a final (non-busy) notification.
///
/// The window comes from the user's configured `ui.status_clear_seconds`, graded
/// by tone, unless `sticky` is set.
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

/// Where a spinner's eventual final is expected to come from, which decides
/// what the leak guard says when it never arrives.
///
/// `wire` is an engine status: the outcome rides the events socket, so silence
/// means dux stopped talking and the log is where the answer is. `local` is a
/// request this tab made itself: the promise is still pending in this very
/// browser, so nothing is lost and there is no server-side log entry to send
/// anyone to. Same guard, two genuinely different facts.
export type BusyOrigin = "wire" | "local"

/// What a spinner is replaced with when its final never arrives.
///
/// Neither wording may claim the operation ended, because nothing here knows
/// that. Both keep the operation's own words so the user can tell which one
/// went quiet, and each says what is actually known about its own kind of
/// silence.
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
/// guard.
///
/// `id` is required rather than optional: a spinner is a thing that gets
/// replaced by its final, and both the replacement and the guard need a name to
/// aim at. `origin` decides only what the guard SAYS; see [`BusyOrigin`].
///
/// NOT sticky, weighed: the guard's warning is a report that a spinner went
/// quiet, and the operation's real final still replaces it in place whenever it
/// turns up. Nothing is lost by letting it retire on the warning window, and
/// pinning one open per stranded spinner is how a flaky connection buries the
/// screen.
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

/// Raise a notification whose tone arrived over the wire as a string.
///
/// This is the engine-status path: the tone is whatever the server sent, `busy`
/// included, and the correlation key is the id so a Busy and its eventual final
/// replace each other in place. A tone this build has never heard of still
/// renders, on the plain success window.
///
/// The tone STRING is deliberately not narrowed to `FinalTone`: an engine status
/// is data, and a build that refuses to show a status it does not recognise is
/// worse than one that shows it neutrally.
///
/// The engine's tones are NOT this module's tones, and the difference is one
/// word. `dux_core`'s status line has no `success`: its `Info` IS the tone a
/// finished operation reports in ("Pulled.", "Changes committed successfully."),
/// so a wire `info` is shown with the success icon. A CLIENT-raised
/// `notifyInfo` keeps the informational icon, because a client that wants to say
/// "this went well" has `notifySuccess` and picks it on purpose.
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
