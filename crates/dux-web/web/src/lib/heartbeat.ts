// THE ONE PERIODIC CLIENT FRAME, and the one timer behind it.
//
// There is exactly one periodic message on a PTY socket and exactly one timer
// driving it. Never two pingers: two timers on one socket is two things to keep
// in step, and the halves genuinely share a cadence.
//
// THE FRAME is `{"beat": <increasing integer>, "viewed": <boolean>}`, answered by
// the server with `{"event":"beat","n":<same>}`.
//
//   `viewed` is the older half and its rule is unchanged: owner AND visible AND
//   outside the attention grace window, decided by `shouldSendViewed` in
//   `viewedPing.ts`. This module calls that; it does not re-decide it.
//
//   `beat` is the liveness half. The server's own WebSocket ping is send-only
//   with no pong deadline, so it reaps a socket the OS has already given up on
//   but cannot see the half-open socket a Wi-Fi to cellular handoff leaves
//   behind, where both ends still believe they are connected. An application
//   number the server echoes gives the browser a round trip it can time out on.
//   A WATCHER sends the frame too, with `viewed: false`: a watcher's socket can
//   go half-open exactly like a driver's, and the beat is not about ownership.
//
// ONE PERIOD, FROM ONE PURE FUNCTION, and it is two values rather than one for a
// reason worth writing down. The viewed ping runs at `VIEWED_PING_INTERVAL_MS`
// (2s) deliberately: it has to stay comfortably under the engine's 3s attention
// window, or a continuously watched agent's flag rises between pings. So the
// period cannot simply be slowed to the heartbeat's. It is 2s while this device
// is owner-and-visible, which is exactly when the viewed half has work to do, and
// `[server] heartbeat_seconds` otherwise, where only liveness is at stake.
//
// EVERY CLOCK HERE IS VISIBLE TIME (see `visibleClock.ts`). A hidden page is
// throttled and a suspended one resumes believing hours passed, so a wall-clock
// deadline would declare a perfectly good socket dead the moment a phone came out
// of a pocket. The frame is only sent while the page is visible, and the answer
// deadline only elapses while it is visible too.
//
// A MISSED ANSWER forces a PLAIN reconnect: drop the socket and let the ordinary
// retry path bring it back. Never a flagged one; an automatic reconnect is never
// a take-over.
import { heartbeatDeadlineMs, heartbeatPeriodMs } from "./connectionTiming"
import { createVisibleClock, type VisibleClock } from "./visibleClock"
import { VIEWED_PING_INTERVAL_MS } from "./viewedPing"

/// How long until the next frame, given what this device is right now. Pure, so
/// the two-value rule above is checkable without a socket.
export function heartbeatIntervalMs(ctx: {
  isOwner: boolean
  visible: boolean
}): number {
  // Owner and visible is the only state that owes the engine a fast viewed ping.
  if (ctx.isOwner && ctx.visible) return VIEWED_PING_INTERVAL_MS
  return heartbeatPeriodMs()
}

export type HeartbeatDeps = {
  /// Put one frame on the wire. Returns whether it actually went out; a frame
  /// the socket discarded starts no deadline, because nothing was asked.
  send: (beat: number, viewed: boolean) => boolean
  /// Whether this device owns the pty, read at each tick.
  isOwner: () => boolean
  /// Whether the `viewed` half should be true, which is `shouldSendViewed`'s
  /// decision and not this module's.
  viewed: () => boolean
  /// The answer never came within the deadline: drop the socket and let the
  /// ordinary retry path reattach, PLAIN.
  onStalled: () => void
  /// Injectable for tests; production reads `document.visibilityState`.
  visible?: () => boolean
  /// Injectable for tests; production reads the configured values.
  periodMs?: (ctx: { isOwner: boolean; visible: boolean }) => number
  deadlineMs?: () => number
  clock?: VisibleClock
}

export type Heartbeat = {
  /// Begin beating. Idempotent.
  start: () => void
  /// Stop and forget any outstanding beat.
  stop: () => void
  /// Feed the server's echo in.
  noteAnswer: (n: number) => void
  /// Forget any outstanding beat without treating it as a miss. The socket
  /// reopened, so the question the old beat asked is moot.
  reset: () => void
}

export function createHeartbeat(deps: HeartbeatDeps): Heartbeat {
  const visible =
    deps.visible ??
    (() =>
      typeof document === "undefined" || document.visibilityState === "visible")
  const period = deps.periodMs ?? heartbeatIntervalMs
  const deadline = deps.deadlineMs ?? heartbeatDeadlineMs
  const clock = deps.clock ?? createVisibleClock()

  let timer: ReturnType<typeof setTimeout> | null = null
  let nextBeat = 1
  // The visible-clock reading when the OLDEST unanswered beat went out, and its
  // number. Kept as the oldest rather than the newest so a run of unanswered
  // beats times out at the deadline rather than at deadline-plus-one-period.
  let pendingSince: number | null = null
  let pendingFrom: number | null = null

  const schedule = () => {
    if (timer !== null) return
    const ms = period({ isOwner: deps.isOwner(), visible: visible() })
    timer = setTimeout(tick, ms)
  }

  const tick = () => {
    timer = null
    // Only while visible. A hidden page sends nothing, and because the clock is
    // paused too, its outstanding beat's deadline cannot elapse while it waits.
    if (!visible()) {
      schedule()
      return
    }
    // The deadline is checked here, on the send timer, rather than on a second
    // timer of its own: one periodic timer is the rule. The cost is that a miss
    // is noticed at the next tick after the deadline passes, which is within one
    // period of it.
    if (pendingSince !== null && clock.elapsedMs() - pendingSince >= deadline()) {
      pendingSince = null
      pendingFrom = null
      deps.onStalled()
      schedule()
      return
    }
    const n = nextBeat++
    if (deps.send(n, deps.viewed()) && pendingSince === null) {
      pendingSince = clock.elapsedMs()
      pendingFrom = n
    }
    schedule()
  }

  const clearPending = () => {
    pendingSince = null
    pendingFrom = null
  }

  return {
    start() {
      schedule()
    },
    stop() {
      if (timer !== null) {
        clearTimeout(timer)
        timer = null
      }
      clearPending()
      if (deps.clock === undefined) clock.dispose()
    },
    noteAnswer(n) {
      // Any answer at or after the oldest outstanding beat proves the round trip
      // works; an answer to something older than what we are waiting on proves
      // nothing about the current wait.
      if (pendingFrom !== null && n >= pendingFrom) clearPending()
    },
    reset() {
      clearPending()
      clock.reset()
    },
  }
}
