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
//   The ghosts retire only on a CONFIRMED change, and are additionally STAMPED
//   with the run identity they were learned under: acting on one requires the
//   current run to be CONFIRMED EQUAL to that stamp. Retiring them outright on
//   an unproven answer costs a returning driver its pty even when the server
//   never moved, which is exactly what happened while this retirement rode the
//   epoch reset, since that fires on every events reconnect and an ordinary
//   mobile drop takes both sockets down together. Refusing to ACT on them while
//   the run is unconfirmed costs the same tap only while the probe cannot
//   answer, and the moment one does the ghosts are usable again.
//
//   THE RESIDUAL THIS CLOSES, rather than trades away. An unknown answer used to
//   keep the ghosts AND let self-succession run on them. A server that really
//   did restart while the probe was unreachable mints connection ids from zero
//   again, so another device's fresh id can equal one of this pane's stale
//   ghosts; the handshake names it, the succession arms with it as
//   `expected_owner`, the server's compare-and-swap matches, and the pane takes
//   a pty it never owned with no press at all. That is the press-less steal the
//   owner's rule forbids, so the rule wins and the tap is the price.
//
//   THIS IS NOT THE VALIDATED GATE, and the two questions must not be merged.
//   `serverValidated.ts` decides whether a socket may ATTACH at all, and there
//   an unknown answer still counts as validated (holding every terminal shut
//   because one endpoint was unreachable is the wrong failure). Here the
//   question is whether a memory keyed to the previous run's counters may be
//   ACTED ON, and an unknown answer is not enough.
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

/// An opaque stamp for "the run this was learned under". Compared, never
/// interpreted; only [`runIdentityConfirmedAs`] gives it meaning.
export type RunStamp = number

/// The run the tab is currently believed to be talking to. It moves only on a
/// CONFIRMED change, because that is the only answer that proves the counters
/// behind every stamped memory were reset.
let runStamp: RunStamp = 1

/// Whether the CURRENT run is confirmed to be `runStamp`. True at load, because
/// the document itself came from the run that served it; false the moment a
/// probe cannot answer, and true again as soon as one says the run is the same.
/// A "same" answer compares against the boot baseline, so it proves the run
/// never moved and re-validates every stamp taken since.
let runConfirmed = true

/// The stamp to record alongside a memory keyed to this run's counters.
export function currentRunStamp(): RunStamp {
  return runStamp
}

/// Whether a memory stamped with `stamp` may be acted on now: the run must be
/// the one it was learned under AND that must be CONFIRMED, not merely
/// unrefuted.
export function runIdentityConfirmedAs(stamp: RunStamp): boolean {
  return runConfirmed && stamp === runStamp
}

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
  if (result === "same") {
    runConfirmed = true
    return
  }
  runConfirmed = false
  if (result === "changed") {
    // A new run means new counters, so every stamp taken so far is stale for
    // good. The page is reloading, but this must hold for whatever is read in
    // the meantime.
    runStamp += 1
    for (const cb of [...changedListeners]) cb()
  }
  for (const cb of [...unconfirmedListeners]) cb()
}
