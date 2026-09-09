// Pure cadence and visibility decisions for the Task Manager's stats poll. Each poll costs
// the server a full process-table walk, so a closed dialog polls nothing and so does a
// backgrounded tab, where the dialog stays mounted but nobody can see it.

/** How often to re-sample while the dialog is open and visible, in ms. Must match the
 * server's resource cache TTL (`CACHE_TTL` in `crates/dux-web/src/resource_routes.rs`);
 * polling faster only re-fetches the same cached sample, and nothing enforces the pair. */
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

// How long to wait before the next sample. The delay shrinks by the time already spent
// fetching, so a slow round-trip does not stretch the cadence and never schedules into the past.
export function nextPollDelay(
  intervalMs: number,
  elapsedMs: number,
): number {
  return Math.max(0, intervalMs - elapsedMs)
}

/** How long the poll may go without a successful sample before the numbers are flagged as
 * stalled. A small multiple of the interval, so one slow request does not flash it. */
export const STALE_STATS_THRESHOLD_MS = RESOURCE_POLL_INTERVAL_MS * 4

// Whether the last successful sample is too old to present as fresh. A `null`
// `lastSuccessAt` is never stale: nothing has landed yet, just the initial dashes.
export function statsAreStale(
  now: number,
  lastSuccessAt: number | null,
  thresholdMs: number = STALE_STATS_THRESHOLD_MS,
): boolean {
  if (lastSuccessAt === null) return false
  return now - lastSuccessAt > thresholdMs
}

// The header's "updating every Ns" pill, derived from the poll constant rather than typed,
// so the copy cannot drift from the actual cadence.
export function pollIntervalLabel(intervalMs: number): string {
  const seconds = intervalMs / 1000
  const formatted = Number.isInteger(seconds) ? String(seconds) : seconds.toFixed(1)
  return `every ${formatted}s`
}
