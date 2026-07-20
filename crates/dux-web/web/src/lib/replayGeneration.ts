// Decision logic for the reconnect-replay idempotency guard (Mechanism A).
//
// On every (re)open the dux web server replays the whole terminal scrollback as one
// Binary blob, tagged with a process-monotonic generation id carried on the
// preceding `connected` handshake frame (see `ptySocket.ts` and the server's
// `handle_pty_socket`). The client records the last generation it applied and drops
// any replay whose generation it has already applied. On mobile the socket
// reconnects constantly (backgrounding, lock, Wi-Fi/cellular handover), so a
// duplicate replay or a late blob from a torn-down forwarder must be a no-op rather
// than a second copy of the scrollback stacked on the buffer (the duplicated-text
// bug). A fresh generation per open makes every legitimate reconnect strictly
// newer, so in normal operation nothing is ever dropped and this guard is inert; it
// fires ONLY on the anomaly.
//
// Kept pure and free of any xterm/DOM dependency so it is unit-testable without
// mounting a terminal.

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

// Fold the last-applied generation forward after a replay is applied. A tagged
// generation advances the high-water mark; an untagged replay (older server) leaves
// it unchanged so a later tagged one still compares sensibly.
export function nextAppliedGeneration(
  gen: number | null | undefined,
  lastAppliedGen: number | null,
): number | null {
  return typeof gen === "number" ? gen : lastAppliedGen
}
