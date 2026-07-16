// Pure cadence/visibility decisions for the Task Manager's stats poll.
//
// The branching lives here, as pure functions, so it is testable without
// mounting the dialog or a live fetch (matching the `viewedPing.ts` /
// `termkeys.ts` convention).
//
// Stats are polled only while the dialog is actually being looked at. Each poll
// costs the server a full process-table walk, so a closed dialog polls nothing
// (that backpressure is the reason stats are REST and not a ws topic) and a
// backgrounded browser tab polls nothing either: the dialog stays mounted when
// the tab is hidden, and walking the process table every second for a screen
// nobody can see is pure waste.

/** How often to re-sample while the dialog is open and visible, in ms.
 *
 * Pinned to the server's resource cache TTL (`CACHE_TTL` in
 * `crates/dux-web/src/resource_routes.rs`): polling faster would only burn
 * requests that are served from the same cached sample. It also gives the
 * collector's CPU delta a full second to span, which is what makes the reading
 * representative. A plain duplicated literal, not generated from the Rust
 * constant, so nothing enforces the two staying equal. */
export const RESOURCE_POLL_INTERVAL_MS = 1000

export interface PollContext {
  /** Whether the Task Manager dialog is open. */
  open: boolean
  /** `document.hidden`: the browser tab is backgrounded. */
  hidden: boolean
}

// Whether a poll should run right now.
export function shouldPoll(ctx: PollContext): boolean {
  return ctx.open && !ctx.hidden
}

// How long to wait before the next sample, given how long the last one took.
//
// Wall-clock based (`Instant::elapsed()` in spirit): the delay shrinks by the
// time already spent fetching, so a slow round-trip does not stretch the cadence
// and a fetch slower than the interval simply polls again immediately rather
// than scheduling into the past.
export function nextPollDelay(
  intervalMs: number,
  elapsedMs: number,
): number {
  return Math.max(0, intervalMs - elapsedMs)
}

/** How long the poll may go without a successful sample before the Task
 * Manager flags the numbers as stalled instead of silently rendering an
 * increasingly-stale last-good reading forever. A small multiple of the poll
 * interval so a single dropped or slow request does not flash the indicator;
 * a genuinely broken poll (server gone, network down) crosses it quickly. */
export const STALE_STATS_THRESHOLD_MS = RESOURCE_POLL_INTERVAL_MS * 4

// Whether the last successful sample is old enough that the numbers on
// screen should no longer be presented as fresh. `lastSuccessAt` is `null`
// before the first sample ever lands, which is never stale: there is nothing
// to judge yet, just the initial dashes.
export function statsAreStale(
  now: number,
  lastSuccessAt: number | null,
  thresholdMs: number = STALE_STATS_THRESHOLD_MS,
): boolean {
  if (lastSuccessAt === null) return false
  return now - lastSuccessAt > thresholdMs
}

// The Task Manager header's "updating every Ns" pill. Derived from the real
// poll constant (never a hand-typed number) so the copy cannot drift from the
// actual cadence if `RESOURCE_POLL_INTERVAL_MS` ever changes.
export function pollIntervalLabel(intervalMs: number): string {
  const seconds = intervalMs / 1000
  const formatted = Number.isInteger(seconds) ? String(seconds) : seconds.toFixed(1)
  return `every ${formatted}s`
}
