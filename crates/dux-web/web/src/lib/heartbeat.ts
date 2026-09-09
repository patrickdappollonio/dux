// The one periodic client frame on a PTY socket, and the one timer behind it.
// Never a second pinger: the two halves of the frame share a cadence.
//
// The frame is `{"beat": <increasing integer>, "viewed": <boolean>}`, answered
// by the server with `{"event":"beat","n":<same>}`.
//
//   `viewed` is decided by `shouldSendViewed` in `viewedPing.ts`; this module
//   calls it and does not re-decide it.
//
//   `beat` is liveness. The server's own WebSocket ping is send-only with no
//   pong deadline, so it cannot see the half-open socket a Wi-Fi to cellular
//   handoff leaves behind; an echoed number gives the browser a round trip to
//   time out on. A watcher sends the frame too, with `viewed: false`, because
//   its socket goes half-open the same way.
//
// The period is two values from one pure function. `VIEWED_PING_INTERVAL_MS`
// must stay under the engine's attention window or a continuously watched
// agent's flag rises between pings, so it holds while this device is
// owner-and-visible; `[server] heartbeat_seconds` applies otherwise, where only
// liveness is at stake.
//
// Every clock here is visible time (`visibleClock.ts`): a hidden page is
// throttled and a suspended one resumes believing hours passed, so a wall-clock
// deadline would kill a healthy socket on a phone leaving a pocket. Frames go
// out only while visible and the deadline elapses only while visible.
//
// A missed answer forces a plain reconnect through the ordinary retry path,
// never a flagged one: an automatic reconnect is never a take-over.
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
  /// Re-read the cadence inputs and, if the period changed, clear the armed
  /// timer and arm the new one; also unparks a heartbeat the page going hidden
  /// parked. Without it a take-over or a return to the tab waits out a whole
  /// slow period past a boundary the engine answers in seconds. Fired by this
  /// module's visibility listener, and by the pane when ownership flips.
  resync: () => void
}

export function createHeartbeat(deps: HeartbeatDeps): Heartbeat {
  const visible =
    deps.visible ??
    (() =>
      typeof document === "undefined" || document.visibilityState === "visible")
  const period = deps.periodMs ?? heartbeatIntervalMs
  const deadline = deps.deadlineMs ?? heartbeatDeadlineMs
  // `start()` rebuilds an owned clock a previous `stop()` disposed: a disposed
  // clock has lost its visibility listener and counts hidden time as visible.
  // An injected clock belongs to the caller and is never disposed here.
  let clock = deps.clock ?? createVisibleClock()
  let clockDisposed = false

  let timer: ReturnType<typeof setTimeout> | null = null
  // The period the ARMED timer was armed with, so a change of cadence can be
  // recognised rather than waited out. Null whenever nothing is armed.
  let armedPeriod: number | null = null
  // Whether `start` has run and `stop` has not. A resync must never resurrect a
  // heartbeat the pane deliberately stopped.
  let running = false
  let nextBeat = 1
  // The visible-clock reading when the OLDEST unanswered beat went out, and its
  // number. Kept as the oldest rather than the newest so a run of unanswered
  // beats times out at the deadline rather than at deadline-plus-one-period.
  let pendingSince: number | null = null
  let pendingFrom: number | null = null
  // Whether the most recent frame reached the wire. A connecting or closed
  // socket discards frames silently, and a discarded frame asks nothing, so no
  // deadline may run against it. Starts true because nothing has failed yet.
  let lastSendReached = true

  const currentPeriod = () =>
    period({ isOwner: deps.isOwner(), visible: visible() })

  const schedule = () => {
    if (!running) return
    if (timer !== null) return
    // Park while hidden: a hidden page sends nothing and its clock is paused, so
    // an armed timer is only a wake-up the platform throttles or drops.
    if (!visible()) {
      armedPeriod = null
      return
    }
    const ms = currentPeriod()
    armedPeriod = ms
    timer = setTimeout(tick, ms)
  }

  const resync = () => {
    if (!running) return
    if (timer === null) {
      // Parked. Scheduling is the whole answer.
      schedule()
      return
    }
    if (!visible()) {
      clearTimeout(timer)
      timer = null
      armedPeriod = null
      return
    }
    if (currentPeriod() === armedPeriod) return
    clearTimeout(timer)
    timer = null
    schedule()
  }

  const onVisibilityChange = () => {
    resync()
  }

  const tick = () => {
    timer = null
    armedPeriod = null
    // The paused clock means an outstanding beat's deadline cannot elapse here.
    if (!visible()) {
      schedule()
      return
    }
    // The deadline is checked on the send timer, since one periodic timer is the
    // rule, so a miss is noticed within one period of it. Only against a socket
    // taking frames: a discarded frame asked nothing (see `lastSendReached`).
    if (
      lastSendReached &&
      pendingSince !== null &&
      clock.elapsedMs() - pendingSince >= deadline()
    ) {
      pendingSince = null
      pendingFrom = null
      deps.onStalled()
      schedule()
      return
    }
    const n = nextBeat++
    const reached = deps.send(n, deps.viewed())
    if (reached) {
      if (pendingSince === null) {
        pendingSince = clock.elapsedMs()
        pendingFrom = n
      }
    } else {
      // Retire whatever was outstanding: it was asked of a connection that is
      // gone, and the next frame that reaches the wire starts a fresh deadline.
      clearPending()
    }
    lastSendReached = reached
    schedule()
  }

  const clearPending = () => {
    pendingSince = null
    pendingFrom = null
  }

  // This module's own visibility listener, so a return to the tab retimes the
  // beat with no help from callers. Guarded on the method rather than the
  // global: this runs off-browser and under harnesses stubbing a partial
  // `document`.
  const canListen =
    typeof document !== "undefined" &&
    typeof document.addEventListener === "function"

  return {
    start() {
      if (running) return
      running = true
      if (clockDisposed) {
        clock = createVisibleClock()
        clockDisposed = false
      }
      lastSendReached = true
      if (canListen) {
        document.addEventListener("visibilitychange", onVisibilityChange)
      }
      schedule()
    },
    stop() {
      running = false
      if (canListen) {
        document.removeEventListener("visibilitychange", onVisibilityChange)
      }
      if (timer !== null) {
        clearTimeout(timer)
        timer = null
      }
      armedPeriod = null
      clearPending()
      if (deps.clock === undefined) {
        clock.dispose()
        clockDisposed = true
      }
    },
    resync,
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
