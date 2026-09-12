// The reconnect loop's arithmetic, with no timers, no sockets and no clock in
// it. `ReconnectingSocket` counts failures and owns the timers; everything about
// how long to wait and whether to keep trying at all is decided here, so the
// shape can be asserted directly rather than inferred from advancing fake time.

/// The first gap, and the one every reset returns to. Half a second is short
/// enough that a server bouncing in place is back before the user notices.
export const RECONNECT_MIN_MS = 500

/// What to do after a failure. `attempt` is the number of the attempt about to
/// be scheduled; `attempts` on the give-up is how many were made in total, which
/// is the number the overlay says out loud.
export type ReconnectStep =
  | { kind: "retry"; attempt: number; delayMs: number }
  | { kind: "give_up"; attempts: number }

export type ReconnectPlanInput = {
  /// How many attempts have failed in a row, counting the one that just did. An
  /// attempt abandoned for never opening is one of these: the caller counts it
  /// before asking, so this side cannot tell the two apart and does not need to.
  failures: number
  /// `[server] reconnect_backoff_cap_seconds` in ms. Read per call, so a config
  /// reload applies to the next gap rather than to the next page load.
  capMs: number
  /// `[server] reconnect_attempts`, where `0` means never give up.
  budget: number
}

/// The gap before the attempt that follows `failures` failures. Doubles from the
/// floor and then holds at the cap. Floored as well as capped, because a cap
/// below the floor would otherwise be a hot retry loop.
export function retryDelayMs(failures: number, capMs: number): number {
  const doubled = RECONNECT_MIN_MS * 2 ** Math.max(0, failures - 1)
  return Math.max(RECONNECT_MIN_MS, Math.min(doubled, capMs))
}

/// Whether this many failures in a row exhausts the budget. A zero budget is
/// never spent: that is how the config says "keep trying for as long as the page
/// is open", which is what dux did before the budget existed.
export function budgetSpent(failures: number, budget: number): boolean {
  if (budget <= 0) return false
  return failures >= budget
}

/// The whole decision after one failure: wait and try again, or stop.
export function planNextAttempt(input: ReconnectPlanInput): ReconnectStep {
  if (budgetSpent(input.failures, input.budget)) {
    return { kind: "give_up", attempts: input.failures }
  }
  return {
    kind: "retry",
    attempt: input.failures + 1,
    delayMs: retryDelayMs(input.failures, input.capMs),
  }
}
