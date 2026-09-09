// The reconnect-replay idempotency guard. Every open replays the whole scrollback as
// one blob, tagged with a process-monotonic generation from the `connected`
// handshake; a replay whose generation was already applied is dropped, so a
// duplicate or a late blob from a torn-down forwarder cannot stack a second copy of
// the scrollback on the buffer. A fresh generation per open makes every legitimate
// reconnect strictly newer, so the guard is inert in normal operation.

export function shouldApplyReplay(
  gen: number | null | undefined,
  lastAppliedGen: number | null,
): boolean {
  // No generation on the wire (an older server that predates the tag): apply, so
  // the guard is backward-safe and never suppresses a legitimate replay.
  if (gen === null || gen === undefined) return true
  // Nothing applied yet on this socket lifetime: the first replay always paints.
  if (lastAppliedGen === null) return true
  // Otherwise only a strictly newer generation paints; an equal or older one is a
  // duplicate or stale blob and is dropped.
  return gen > lastAppliedGen
}

// Folds the last-applied generation forward. An untagged replay leaves the
// high-water mark unchanged, so a later tagged one still compares sensibly.
export function nextAppliedGeneration(
  gen: number | null | undefined,
  lastAppliedGen: number | null,
): number | null {
  return typeof gen === "number" ? gen : lastAppliedGen
}
