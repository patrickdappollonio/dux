// ACCUMULATED VISIBLE TIME, the only clock the connection machinery waits on.
//
// Every wait in the attach path (the replay cover's patience, the heartbeat's
// answer deadline) is a question about how long the USER has been looking at a
// stalled pane, not about how long the wall clock ran. A bare `setTimeout`
// answers the second question badly and the first one not at all:
//
//   - A backgrounded tab is throttled. Chrome clamps timers in a hidden page to
//     roughly one per minute and freezes them outright after a few minutes, so
//     an 8 second wait can fire a minute late or never.
//   - A suspended page (a phone locked in a pocket, a laptop lid closed) resumes
//     with a timer that believes hours passed, so the first thing the returning
//     user sees is a wait that has already expired against a socket that never
//     had a chance.
//
// So both clocks sum `performance.now()` deltas across VISIBLE spans only and
// pause while hidden. `performance.now()` rather than `Date.now()` because it is
// monotonic: a clock adjustment or an NTP step cannot make an elapsed reading go
// backwards or leap forward.
//
// The arithmetic is pure (three functions, no DOM), and the small factory below
// is the only part that touches `document`.

/// One accumulator: the visible milliseconds already banked, plus the start of
/// the visible span currently running (`null` while hidden).
export type VisibleSpan = {
  banked: number
  runningSince: number | null
}

/// A fresh accumulator at zero. `visible` seeds whether a span is already
/// running, so a clock created on a hidden page counts nothing until it returns.
export function freshSpan(now: number, visible: boolean): VisibleSpan {
  return { banked: 0, runningSince: visible ? now : null }
}

/// The visible milliseconds elapsed so far: what is banked, plus the span still
/// running (if any).
export function elapsedVisibleMs(span: VisibleSpan, now: number): number {
  if (span.runningSince === null) return span.banked
  return span.banked + (now - span.runningSince)
}

/// Fold one visibility sample in. Going hidden banks the running span; becoming
/// visible starts a new one. A REDUNDANT sample (the state the span is already
/// in) is deliberately a no-op rather than a restart, because several return
/// signals commonly fire in the same tick and restarting on each would rebank
/// the same span twice.
export function afterVisibilitySample(
  span: VisibleSpan,
  now: number,
  visible: boolean,
): VisibleSpan {
  if (visible) {
    if (span.runningSince !== null) return span
    return { banked: span.banked, runningSince: now }
  }
  if (span.runningSince === null) return span
  return { banked: span.banked + (now - span.runningSince), runningSince: null }
}

/// A live accumulator wired to `document`'s visibility.
export type VisibleClock = {
  /// Accumulated visible milliseconds since construction or the last `reset`.
  elapsedMs: () => number
  /// Start a new epoch from zero. Every attach epoch gets one, because the
  /// previous open's patience says nothing about this one's.
  reset: () => void
  /// Detach the visibility listener. The reading is not frozen; the span it was
  /// left in keeps running, which is what a disposed clock nobody reads costs.
  dispose: () => void
}

/// Build a clock over `document.visibilityState`. A context with no document
/// (a non-jsdom unit test, SSR) reads as visible, which is the same
/// never-silently-suppress default `isForeground` takes.
export function createVisibleClock(): VisibleClock {
  const visible = () =>
    typeof document === "undefined" || document.visibilityState === "visible"
  let span = freshSpan(performance.now(), visible())
  const onVisibility = () => {
    span = afterVisibilitySample(span, performance.now(), visible())
  }
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibility)
  }
  return {
    elapsedMs: () => elapsedVisibleMs(span, performance.now()),
    reset: () => {
      span = freshSpan(performance.now(), visible())
    },
    dispose: () => {
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", onVisibility)
      }
    },
  }
}
