// WHAT THE RUN-IDENTITY PROBE ANSWERED, published to the few things that keep a
// memory of the server's own counters.
//
// The store asks, on every events-socket reconnect, whether this is still the
// run that served this tab (see `reloadIfServerChanged`). A CHANGED answer hard
// reloads the page, so most of the app never has to think about it. Two client
// memories do, because both are keyed to counters a restarted server restarts
// from zero, and both are read before the reload can possibly land:
//
//   THE PANE'S GHOST CONNECTION IDS. Self-succession recognises this pane's own
//   dead connection in a handshake's owner field and re-claims the pty. On a new
//   run, the id it remembers may have been minted afresh for somebody else's
//   connection, so succeeding on it would take a pty this pane never owned.
//
//   THE APPLIED REPLAY GENERATION. The replay dedupe drops any generation it has
//   already applied. A new run's generations start low, so a surviving
//   high-water mark drops the new run's replay whole and the cover clears over
//   the PREVIOUS run's screen.
//
// THEY SUBSCRIBE TO DIFFERENT ANSWERS, deliberately, because their risks are not
// symmetric.
//
//   The ghosts retire only on a CONFIRMED change. Retiring them on an unproven
//   one costs a returning driver its pty: it lands as a watcher and has to tap
//   Take over, which is exactly what happened while this retirement rode the
//   epoch reset, since that fires on every events reconnect and an ordinary
//   mobile drop takes both sockets down together.
//
//   The generation retires on a change OR an UNVERIFIED answer. A failed probe
//   is precisely the case where a restarted server goes unnoticed, and retiring
//   a high-water mark costs nothing in normal operation: within one run every
//   new open carries a strictly greater generation, so the mark it forgets was
//   never going to drop anything anyway.
//
// This is not the run-identity policy itself, which lives in `serverIdentity.ts`
// and the store. It is only the fan-out, kept in a leaf module so a pane can
// subscribe without importing the store (which imports the pane's sockets).

/// What the probe said about the run behind this reconnect.
export type ServerRunProbe =
  /// Same build, same process id: nothing to retire.
  | "same"
  /// A different run answered. The page is being reloaded; whatever is read
  /// between now and then must not trust the old run's counters.
  | "changed"
  /// The probe could not say (it failed, or the server does not answer). Not
  /// evidence of a change, but not evidence of sameness either.
  | "unknown"

const changedListeners = new Set<() => void>()
const unconfirmedListeners = new Set<() => void>()

/// Subscribe to a CONFIRMED run change. Returns the unsubscribe.
export function onServerRunChanged(cb: () => void): () => void {
  changedListeners.add(cb)
  return () => {
    changedListeners.delete(cb)
  }
}

/// Subscribe to "this run is not confirmed to be the one we started with": a
/// change, or a probe that could not answer. Returns the unsubscribe.
export function onServerRunUnconfirmed(cb: () => void): () => void {
  unconfirmedListeners.add(cb)
  return () => {
    unconfirmedListeners.delete(cb)
  }
}

/// Publish a probe result. Called by the store, from the one place that runs the
/// probe. Snapshots each listener set so a callback that unsubscribes during
/// dispatch cannot perturb the live iteration.
export function noteServerRunProbe(result: ServerRunProbe): void {
  if (result === "same") return
  if (result === "changed") {
    for (const cb of [...changedListeners]) cb()
  }
  for (const cb of [...unconfirmedListeners]) cb()
}
