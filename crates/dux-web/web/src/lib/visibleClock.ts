// Accumulated visible time, the only clock the connection machinery waits on: every
// wait in the attach path asks how long the user has been looking at a stalled pane,
// which a bare timer cannot answer. A hidden tab is throttled or frozen, and a
// suspended page resumes believing hours passed.
//
// So the clocks sum `performance.now()` deltas across visible spans and pause while
// hidden. `performance.now()` because it is monotonic: a clock adjustment cannot make
// an elapsed reading go backwards or leap forward.

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

/// Folds one visibility sample in: going hidden banks the running span, becoming
/// visible starts a new one. A sample matching the current state is deliberately a
/// no-op, since several return signals fire in one tick and would rebank the span.
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
