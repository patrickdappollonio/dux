// Publishes run-identity probe results to pane state keyed by server counters.
// Ghost connection ids may be used only when their run stamp is confirmed;
// replay-generation high-water marks retire whenever the run is not confirmed
// unchanged. Socket admission is a separate policy in `serverValidated.ts`.

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
