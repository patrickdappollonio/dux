// The server's run clock, as last read from a delivery of the workspace
// document. A verdict says when its run ended on that clock, a value that never
// changes, so the server does not republish an aging verdict; the age is worked
// out here instead: what the clock read when the document was sent, plus the
// time that has passed on this page's own monotonic clock since it arrived.
// Nothing compares this machine's clock with the server's.
//
// Every delivery re-reads it (the boot fetch, every pushed frame, and the
// replay a reconnecting socket is handed), so a page that slept or lost its
// socket is put right by the document it receives next.

interface Reading {
  /** The server's run clock, in whole seconds, when the document was sent. */
  server: number
  /** `performance.now()` when it arrived. */
  receivedAt: number
}

let reading: Reading | null = null

/** Note the run clock a document arrived with. Anything but a finite number is
 * ignored, so a delivery that did not carry one keeps the last good reading. */
export function noteServerClock(seconds: unknown): void {
  if (typeof seconds !== "number" || !Number.isFinite(seconds)) return
  reading = { server: seconds, receivedAt: performance.now() }
}

/** How many seconds ago `endedAt` (a run-clock reading) was, or `null` before
 * any document has said what the clock reads. */
export function runClockAgeSeconds(endedAt: number): number | null {
  if (reading === null) return null
  return reading.server - endedAt + (performance.now() - reading.receivedAt) / 1000
}

/** Forget the reading, for tests. */
export function resetServerClockForTests(): void {
  reading = null
}
