import {
  reconnectAttemptBudget,
  reconnectAttemptTimeoutMs,
  reconnectBackoffCapMs,
} from "./connectionTiming"
import { RECONNECT_MIN_MS, planNextAttempt, retryDelayMs } from "./reconnectSchedule"
import type { ConnState } from "./types"

// Events and PTY sockets retry network failures with capped backoff, bounded by
// the configured attempt budget. Terminal close codes fail; wake signals are
// idempotent and health resets the schedule.
export { RECONNECT_MIN_MS }

/// What the socket is doing about the connection right now, published through
/// `onPlan` so a surface can say it out loud rather than showing an
/// indeterminate spinner over an unknown wait.
export type ReconnectPhase = "waiting" | "connecting" | "given_up" | "open"

export type ReconnectPlanEvent = {
  phase: ReconnectPhase
  /// The attempt this phase is about: the one that just failed while waiting,
  /// the one in flight while connecting, the number made in total on a give-up,
  /// and the one that succeeded on an open. Zero only before anything has
  /// failed, which a gate-held first arming can produce.
  attempt: number
  /// The configured budget, where `0` means unlimited. Carried so a surface can
  /// say "of 8" without reading config itself.
  budget: number
  /// `Date.now()` of the next attempt while waiting; null in every other phase.
  /// Wall clock rather than a countdown, so a surface ticking once a second
  /// stays honest whatever its own cadence does.
  nextAttemptAt: number | null
  /// How long the attempt in flight (or the next one) may sit unopened.
  attemptTimeoutMs: number
}

/// How long a socket must STAY open before the open counts as evidence that the
/// connection works. Receiving a frame proves health immediately.
export const HEALTHY_SETTLE_MS = 2_000

/// The two behaviors a subclass chooses. Both default to the events socket's
/// answer, so a socket that says nothing behaves exactly as that one does.
export type ReconnectPolicy = {
  /// Stop scheduling retries while the document is hidden. PTY sockets only;
  /// see the module doc.
  parkWhileHidden: boolean
  /// Consulted immediately before every attempt, wake signals included.
  /// Returning false holds the retry (the timer re-arms) rather than ending it,
  /// so the socket resumes on its own the moment the gate opens. The PTY socket
  /// gates on the server-run identity check having resolved, because attaching
  /// to a restarted server force-launches a provider.
  canRetry: () => boolean
  /// The backoff ceiling, read at each doubling so a config change applies to
  /// the next gap rather than to the next page load.
  backoffCapMs: () => number
  /// How many consecutive failed attempts end the loop, where `0` is never.
  /// Consulted per failure, so a config reload applies to the outage in
  /// progress. The events socket takes the configured budget, because it is the
  /// one connection whose absence the user can see and act on; a PTY socket
  /// passes `0` and leans on `canRetry`, which already holds it shut for as long
  /// as the events socket is down.
  attemptBudget: () => number
}

// The shared reconnecting WebSocket base. Subclasses supply the socket-specific
// bits through the protected hooks below; everything to do with *when* to
// reconnect and *what connection state to emit* is owned here.
export abstract class ReconnectingSocket {
  protected url: string
  protected ws: WebSocket | null = null
  // Consecutive attempts that failed, an attempt abandoned for never opening
  // included. Zeroed by a healthy open and by `connect()`; the whole input to
  // the schedule.
  private failures = 0
  // The number of the attempt in flight, for the plan a surface renders.
  private attempt = 0
  // The budget is spent: no timer is armed and nothing automatic will try again.
  // Distinct from `stopped`, which is the far end saying do not come back; this
  // one a wake signal revives.
  private givenUp = false
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  // Armed while a socket is CONNECTING; see `reconnectAttemptTimeoutMs`.
  private connectTimer: ReturnType<typeof setTimeout> | null = null
  // Armed from an open until that open has earned the reset; see
  // `HEALTHY_SETTLE_MS`.
  private settleTimer: ReturnType<typeof setTimeout> | null = null
  protected closedByUser = false
  // The far end said "do not come back": a terminal close code, or a route this
  // client knows is gone. Distinct from `closedByUser`, and the one state no
  // wake signal may revive.
  private stopped = false
  // Disposed is the one state nothing revives, `connect()` included. `stopped`
  // is deliberately cleared by `connect()`, because a terminal close code is
  // recoverable by the Reconnect button; disposal is not, because the pane that
  // owned this socket is gone.
  private disposed = false
  private readonly policy: ReconnectPolicy
  private wakeAttached = false

  // Connection-state transitions ("connecting" | "open" | "closed" | "failed").
  // Drives the status indicator / offline modal (events socket) and the focused
  // terminal's cover (PTY socket).
  onConn: (state: ConnState) => void = () => {}
  // Fired after the socket (re)opens and the subclass's `onSocketOpen` hook has
  // run, so the consumer can re-fetch or re-arm after every open.
  onOpen: () => void = () => {}
  // Fired once per drop when a reconnect is intended, never on a user-initiated
  // `close()` and never when the far end said stop. Fired for a parked socket
  // too: parking changes when the next attempt happens, not whether one is coming.
  onReconnecting: () => void = () => {}
  // The retry schedule, published on every arming, every attempt, the give-up
  // and every open. The offline overlay reads it to say which attempt failed and
  // when the next one is due.
  onPlan: (plan: ReconnectPlanEvent) => void = () => {}

  constructor(url: string, policy: Partial<ReconnectPolicy> = {}) {
    this.url = url
    this.policy = {
      parkWhileHidden: policy.parkWhileHidden ?? false,
      canRetry: policy.canRetry ?? (() => true),
      backoffCapMs: policy.backoffCapMs ?? reconnectBackoffCapMs,
      attemptBudget: policy.attemptBudget ?? reconnectAttemptBudget,
    }
  }

  // A deliberate, user-initiated (re)entry: reset the backoff, `closedByUser` and
  // the stop flag so a fresh connect never inherits a grown delay or a give-up.
  // Also the manual "Reconnect" path and the take-over bounce.
  connect(): void {
    if (this.disposed) {
      // Debug rather than a warning: a late `connect()` on a disposed socket is
      // an ordering artefact of an unmount, not a fault the user can act on.
      console.debug("[dux] ignoring connect() on a disposed socket", this.url)
      return
    }
    this.closedByUser = false
    this.stopped = false
    this.givenUp = false
    this.failures = 0
    this.clearRetryTimer()
    this.attachWakeSignals()
    // Explicit connects obey the same identity gate as automatic retries. A
    // closed gate defers the attach and polls without growing the fresh backoff.
    if (!this.policy.canRetry()) {
      this.armHeldRetry()
      return
    }
    this.open()
  }

  // A page-lifecycle return (`pageshow`, Chromium's `resume`) or one of the wake
  // signals: attempt now, plain, idempotently. Unlike `connect()` it never touches
  // a socket that is live or connecting, and it keeps the grown backoff for the
  // next failure, because a return signal is evidence about the device rather than
  // about the server. It does clear `closedByUser`, the other half of the
  // deliberate `pagehide` close.
  resumeNow(): void {
    if (this.stopped) return
    // Live or still connecting: there is nothing to resume, and tearing it down
    // is exactly the harm this method exists to avoid.
    if (this.ws !== null) return
    this.attachWakeSignals()
    // Resume signals can precede visibility. A parking socket waits until the
    // page is visible so its first resize can establish PTY ownership.
    if (this.parked()) return
    this.closedByUser = false
    // A wake signal is the one thing that revives a spent budget: the page came
    // back, or the device did, which is new evidence that the failures behind it
    // may no longer describe the world. Silence in a visible tab is not, or the
    // give-up would be a slower spinner rather than a stop.
    if (this.givenUp) {
      this.givenUp = false
      this.failures = 0
    }
    if (!this.policy.canRetry()) {
      // The gate is shut: fall back to the ordinary polling retry rather than
      // opening, since a return signal is not permission to attach to a server
      // whose identity has not been confirmed. Repeated wake signals leave an
      // armed timer unchanged and never grow the delay.
      if (this.reconnectTimer === null) this.armHeldRetry()
      return
    }
    this.clearRetryTimer()
    this.open()
  }

  // Treat the live connection as dead and let the ordinary retry path bring it
  // back: close the socket without setting `closedByUser`, so its own `onclose`
  // runs, `onConn("closed")` is emitted and the backoff schedule takes over.
  // Going through the real close rather than through `connect()` keeps the
  // reattach plain (a `connect()` detaches its orphan silently, so nothing would
  // broadcast the close that retires an armed take-over) and gives consumers the
  // same event a network drop produces.
  //
  // Only an open socket can be declared quiet; closing a connecting attempt
  // would restart a retry already in progress.
  dropForRetry(): void {
    const ws = this.ws
    if (ws === null) return
    if (ws.readyState !== WebSocket.OPEN) return
    ws.close()
  }

  // Chromium's `freeze`: cancel anything armed. The page is about to stop
  // executing, and a timer that survives it would fire against a document that
  // has been discarded or resumed hours later.
  park(): void {
    this.clearRetryTimer()
  }

  private open(): void {
    // A socket may already be live here (a double `connect()`, from Reconnect
    // pressed twice mid-reconnect). Detach the orphan's handlers and close it
    // before assigning the new socket: otherwise its later `onclose` nulls the
    // shared `this.ws` and permanently kills outbound frames, with no error.
    if (this.ws !== null) {
      const orphan = this.ws
      this.clearSettleTimer()
      orphan.onopen = null
      orphan.onmessage = null
      orphan.onclose = null
      orphan.onerror = null
      this.ws = null
      orphan.close()
    }
    this.attempt = this.failures + 1
    this.onConn("connecting")
    this.emitPlan("connecting", this.attempt, null)
    const ws = new WebSocket(this.url)
    this.configureSocket(ws)
    this.ws = ws
    this.armConnectTimer(ws)

    ws.onopen = () => {
      // Identity guard: only the socket that is still the live `this.ws` may
      // mutate shared connection state. A late callback from a socket a newer
      // open() already replaced must be inert.
      if (this.ws !== ws) return
      this.clearConnectTimer()
      // An open that lasts means the connection is usable again, so the next drop
      // starts a fresh retry schedule from the floor. One that does not is no
      // evidence, and resetting on it pins the gap at the floor forever.
      this.armHealthySettle()
      this.onSocketOpen()
      this.onConn("open")
      this.emitPlan("open", this.attempt, null)
      this.onOpen()
    }

    ws.onmessage = (event) => {
      if (this.ws !== ws) return
      // A frame crossed the connection: that is the proof the settle window is
      // waiting for, so stop waiting.
      this.markHealthy()
      this.handleMessage(event)
    }

    ws.onclose = (event) => {
      // Only the live socket nulls the shared ref and drives reconnect: without
      // this check an orphan's close would null the live `this.ws`, silently
      // dropping every later outbound frame.
      if (this.ws !== ws) return
      this.clearConnectTimer()
      // This open never earned the reset. Left armed, the timer would fire during
      // the very wait it is supposed to be lengthening.
      this.clearSettleTimer()
      this.ws = null
      this.onConn("closed")
      if (this.closedByUser) return
      // A server may close with an app-specific code meaning "do not retry", such
      // as a PTY whose provider is gone, where re-subscribing would relaunch a
      // doomed provider. `shouldReconnect()` surfaces the stop state and returns
      // false for those; any other close retries, until the budget says stop.
      if (!this.shouldReconnect(event.code)) {
        this.stopped = true
        return
      }
      this.scheduleReconnect()
    }

    // `onerror` is followed by `onclose`; let the close handler drive reconnect.
    ws.onerror = (event) => {
      this.handleError(event)
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return
    // One more attempt has failed, however it failed: a refused connection and
    // an attempt abandoned for never opening spend the budget alike.
    this.failures++
    // Signal the consumer so it can show a non-blocking "Reconnecting…" state.
    // Once per drop: the timer guard above keeps a single retry in flight.
    this.onReconnecting()
    // Parked: schedule nothing at all. A hidden page's timer is throttled to
    // roughly one fire a minute and frozen outright after a few, so an armed retry
    // there is a promise the platform will not keep. A wake signal picks it up.
    // A parked socket does not give up either: a phone in a pocket has been told
    // nothing about the server.
    if (this.parked()) return
    const step = planNextAttempt({
      failures: this.failures,
      capMs: this.policy.backoffCapMs(),
      budget: this.policy.attemptBudget(),
    })
    if (step.kind === "give_up") {
      // No timer, and a terminal connection state: nothing automatic happens
      // from here until the button or a wake signal says otherwise.
      this.givenUp = true
      this.clearRetryTimer()
      this.onConn("failed")
      this.emitPlan("given_up", step.attempts, null)
      return
    }
    this.armRetryTimer(step.delayMs)
  }

  private parked(): boolean {
    return (
      this.policy.parkWhileHidden &&
      typeof document !== "undefined" &&
      document.visibilityState === "hidden"
    )
  }

  // Re-arm at the CURRENT failure count's delay, spending no doubling. Every
  // arming the gate caused rather than a failure goes through here: the backoff
  // measures how badly the far end is answering, so a gate-held socket polls
  // steadily instead of drifting out to the cap.
  private armHeldRetry(): void {
    this.armRetryTimer(retryDelayMs(this.failures, this.policy.backoffCapMs()))
  }

  // Arm the next attempt at the delay the schedule chose, and publish it.
  private armRetryTimer(delay: number): void {
    this.emitPlan("waiting", this.failures, Date.now() + delay)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (this.closedByUser || this.stopped) return
      if (this.parked()) return
      // The gate is consulted at the last possible moment rather than at schedule
      // time: whether the identity check has resolved is a fact about now, and a
      // retry it holds re-arms rather than ending, so it resumes unprompted.
      if (!this.policy.canRetry()) {
        this.armHeldRetry()
        return
      }
      this.open()
    }, delay)
  }

  // Publish the schedule. The timeout and the budget are read here rather than
  // captured, so a config reload reaches the next plan a surface renders.
  private emitPlan(
    phase: ReconnectPhase,
    attempt: number,
    nextAttemptAt: number | null,
  ): void {
    this.onPlan({
      phase,
      attempt,
      budget: this.policy.attemptBudget(),
      nextAttemptAt,
      attemptTimeoutMs: reconnectAttemptTimeoutMs(),
    })
  }

  // Abandon a socket that has sat in CONNECTING past the deadline and let the
  // ordinary retry path bring it back. The orphan's handlers are detached first,
  // as `open()` does, so a late callback can never touch shared state.
  private armConnectTimer(ws: WebSocket): void {
    this.clearConnectTimer()
    this.connectTimer = setTimeout(() => {
      this.connectTimer = null
      if (this.ws !== ws) return
      ws.onopen = null
      ws.onmessage = null
      ws.onclose = null
      ws.onerror = null
      this.ws = null
      this.clearSettleTimer()
      ws.close()
      this.onConn("closed")
      if (this.closedByUser || this.stopped) return
      this.scheduleReconnect()
    }, reconnectAttemptTimeoutMs())
  }

  private clearConnectTimer(): void {
    if (this.connectTimer !== null) {
      clearTimeout(this.connectTimer)
      this.connectTimer = null
    }
  }

  // Start the clock on the current open. `markHealthy` is what it eventually
  // calls, so a frame arriving first simply retires it early.
  private armHealthySettle(): void {
    this.clearSettleTimer()
    this.settleTimer = setTimeout(() => {
      this.settleTimer = null
      this.markHealthy()
    }, HEALTHY_SETTLE_MS)
  }

  private clearSettleTimer(): void {
    if (this.settleTimer !== null) {
      clearTimeout(this.settleTimer)
      this.settleTimer = null
    }
  }

  private clearRetryTimer(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
  }

  // The wake signals. Attached on the first `connect()` and detached only by
  // `dispose()`, so a socket the app really tore down cannot be revived by a
  // window event, while one merely closed by the page lifecycle still hears the
  // return that follows.
  private attachWakeSignals(): void {
    if (this.wakeAttached) return
    // Guard on the methods rather than on the globals: this runs off-browser and
    // under harnesses that stub a partial `document`, and a socket that cannot
    // listen must simply not listen rather than throw on its way to opening.
    if (
      typeof window === "undefined" ||
      typeof document === "undefined" ||
      typeof document.addEventListener !== "function" ||
      typeof window.addEventListener !== "function"
    ) {
      return
    }
    this.wakeAttached = true
    document.addEventListener("visibilitychange", this.onVisibilityWake)
    window.addEventListener("pageshow", this.onWake)
    window.addEventListener("focus", this.onWake)
    window.addEventListener("online", this.onWake)
  }

  private detachWakeSignals(): void {
    if (!this.wakeAttached) return
    this.wakeAttached = false
    document.removeEventListener("visibilitychange", this.onVisibilityWake)
    window.removeEventListener("pageshow", this.onWake)
    window.removeEventListener("focus", this.onWake)
    window.removeEventListener("online", this.onWake)
  }

  // Arrow properties, so the same function identity is added and removed and so
  // `this` is the socket rather than the event target.
  private readonly onWake = (): void => {
    this.resumeNow()
  }

  private readonly onVisibilityWake = (): void => {
    // Going hidden is not a wake. A parking socket answers it by simply not
    // scheduling anything on the next drop.
    if (document.visibilityState !== "visible") return
    this.resumeNow()
  }

  // A lifecycle close: the socket goes down deliberately and stays down until
  // something says otherwise, while this object and its page are still in use.
  // `pagehide` is the caller that matters, and the wake signals are what bring the
  // socket back afterwards, so they stay attached: detaching them leaves a page
  // that returns through anything other than `pageshow` with dead sockets and no
  // way back but the Reconnect button.
  close(): void {
    this.closedByUser = true
    this.clearRetryTimer()
    this.clearConnectTimer()
    this.clearSettleTimer()
    this.ws?.close()
  }

  // The real teardown: this socket will never be used again (its pane unmounted,
  // or switched to a different target). Everything `close()` does, plus the wake
  // listeners, which is the difference between the two.
  dispose(): void {
    this.disposed = true
    this.stopped = true
    this.detachWakeSignals()
    this.close()
  }

  // Reset the backoff when the connection is confirmed usable, so the next drop
  // starts from the minimum delay. The base calls it from `onopen` for sockets
  // whose open proves usability; a subclass whose open does not calls it from its
  // own readiness signal instead.
  protected markHealthy(): void {
    this.clearSettleTimer()
    this.failures = 0
  }

  // ---- Subclass extension hooks ----------------------------------------------

  // Tweak the freshly-constructed WebSocket before handlers are attached. Default:
  // no-op. (`void ws` keeps the param in the base signature without tripping
  // no-unused-vars.)
  protected configureSocket(ws: WebSocket): void {
    void ws
  }

  // Run subclass-specific work on every (re)open, before `onOpen` fires.
  protected abstract onSocketOpen(): void

  // Handle one server frame. EventsSocket parses text; PtySocket splits binary
  // (PTY bytes) from the text control frames.
  protected abstract handleMessage(event: MessageEvent): void

  // Consulted on every unexpected close, before scheduling a reconnect, with the
  // close code. Returning `false` stops the loop for good. Default: always
  // reconnect, whatever the code.
  protected shouldReconnect(closeCode: number): boolean {
    void closeCode
    return true
  }

  // React to the socket's `error` event. Default: no-op. (`void event` keeps the
  // param without tripping the unused-vars lint.)
  protected handleError(event: Event): void {
    void event
  }
}
