import { reconnectBackoffCapMs } from "./connectionTiming"
import type { ConnState } from "./types"

// The single home for the frontend's WebSocket reconnect behavior. Both the
// `/ws/events` JSON spine socket (`EventsSocket`) and every per-PTY byte socket
// (`PtySocket`) extend this base so the lifecycle lives in exactly one place and
// can never drift between the two connections.
//
// RECONNECTING IS INDEFINITE. Retry with exponential backoff from
// `RECONNECT_MIN_MS`, doubling to a CONFIGURABLE ceiling
// (`[server] reconnect_backoff_cap_seconds`) and then staying there, forever,
// while the page is visible. There is no attempt budget and no give-up state.
//
// `failed` is therefore reserved for a TERMINAL close code, decided by
// `shouldReconnect`: a provider that will not launch, a deleted tab's route that
// will 404 forever. Those are facts about the far end, not about the network.
//
// A HIDDEN PAGE PARKS, but only where parking is the right answer. It is a
// POLICY the subclass chooses, and it is PTY-ONLY: attention indicators and OS
// notifications ride the events socket precisely while the tab is in the
// background, so that socket keeps retrying at the cap while hidden. A parked
// socket schedules nothing and burns no timers, which is also the only honest
// thing to do on a platform that would throttle or freeze the timer anyway.
//
// FOUR WAKE SIGNALS unpark it: `visibilitychange` to visible, `pageshow`
// (bfcache restore, persisted or not), `focus`, and `online`. Each is a request
// to attempt NOW and every one of them is idempotent, because a phone coming
// back commonly fires three or four in the same tick and a `connect()` on a live
// socket would tear down a working connection each time. None of them opens a
// PARKING socket while the page is still hidden: several of them (Chromium's
// `resume` above all) routinely arrive ahead of the page being on screen, and an
// attach that lands hidden claims nothing and is never re-asked. See
// `resumeNow`.
//
// AN OPEN IS NOT PROOF THE CONNECTION WORKS. It has to last, or carry a frame,
// before the backoff is allowed to start again from the floor; see
// `HEALTHY_SETTLE_MS`.
export const RECONNECT_MIN_MS = 500

/// How long a socket may sit in CONNECTING before it is abandoned and retried.
///
/// A socket that never opens and never errors is the one state nothing else here
/// can rescue: `resumeNow` returns early while `this.ws` is non-null, precisely
/// so a wake signal cannot tear down a connection that is working, so all four
/// wake signals are inert against it and the retry timer is not armed. That is a
/// pane covered forever on a page whose user is pressing every button they have.
///
/// A client-side CONSTANT rather than a config key, deliberately. It is not a
/// policy anybody would want to tune: it is a floor under a platform behavior,
/// and it is set far above any healthy WebSocket handshake (which completes in
/// milliseconds on a working link and in a couple of seconds on a bad one) so a
/// slow network can never trip it.
export const CONNECT_TIMEOUT_MS = 30_000

/// How long a socket must STAY open before the open counts as evidence that the
/// connection works and the backoff may start again from the floor.
///
/// An open is a promise, not a proof. A server that accepts the handshake and
/// drops the connection in the same breath (a proxy with nothing behind it, a
/// server mid-restart, a captive portal) produced an open every time, and the
/// backoff reset on every one of them: measured, the client retried at a flat
/// ~551ms forever and the growth the backoff exists for never happened once.
///
/// A frame arriving is the better proof and it short-circuits this window; the
/// timer is what covers a connection that is genuinely up and simply quiet,
/// which is the ordinary state of an idle PTY.
///
/// A client-side CONSTANT for the same reason as the connect deadline: it is a
/// floor under a platform behavior rather than a policy anybody would tune.
export const HEALTHY_SETTLE_MS = 2_000

/// The two behaviors a subclass chooses. Both default to the events socket's
/// answer, so a socket that says nothing behaves exactly as that one does.
export type ReconnectPolicy = {
  /// Stop scheduling retries while the document is hidden. PTY sockets only;
  /// see the module doc.
  parkWhileHidden: boolean
  /// Consulted immediately before every attempt, wake signals included.
  /// Returning false HOLDS the retry (the timer re-arms) rather than ending it,
  /// so the socket resumes on its own the moment the gate opens. The PTY socket
  /// gates on the server-run identity check having RESOLVED, because attaching
  /// to a restarted server force-launches a provider.
  canRetry: () => boolean
  /// The backoff ceiling, read at each doubling so a config change applies to
  /// the next gap rather than to the next page load.
  backoffCapMs: () => number
}

// The shared reconnecting WebSocket base. Subclasses supply the socket-specific
// bits through the protected hooks below; everything to do with *when* to
// reconnect and *what connection state to emit* is owned here.
export abstract class ReconnectingSocket {
  protected url: string
  protected ws: WebSocket | null = null
  private reconnectDelay = RECONNECT_MIN_MS
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  // Armed while a socket is CONNECTING; see `CONNECT_TIMEOUT_MS`.
  private connectTimer: ReturnType<typeof setTimeout> | null = null
  // Armed from an open until that open has earned the reset; see
  // `HEALTHY_SETTLE_MS`.
  private settleTimer: ReturnType<typeof setTimeout> | null = null
  protected closedByUser = false
  // The far end said "do not come back": a terminal close code, or a route this
  // client knows is gone. Distinct from `closedByUser`, and the one state no
  // wake signal may revive.
  private stopped = false
  // DISPOSED IS THE ONE STATE NOTHING REVIVES, `connect()` included. `stopped`
  // is deliberately cleared by `connect()`, because a terminal close code is
  // recoverable by the Reconnect button; disposal is not recoverable by
  // anything, because the pane that owned this socket is gone. Reviving it would
  // open a connection nothing reads and, for a PTY, launch a provider for a pane
  // that unmounted.
  private disposed = false
  private readonly policy: ReconnectPolicy
  private wakeAttached = false

  // Connection-state transitions ("connecting" | "open" | "closed" | "failed").
  // Drives the status indicator / offline modal (events socket) and the focused
  // terminal's cover (PTY socket).
  onConn: (state: ConnState) => void = () => {}
  // Fired after the socket (re)opens AND the subclass's `onSocketOpen` hook has
  // run (EventsSocket resends its subscription set; PtySocket has nothing to
  // resend). Lets the consumer re-fetch / re-arm after every open.
  onOpen: () => void = () => {}
  // Fired once per drop, when a reconnect is intended (NOT on a user-initiated
  // `close()`, and NOT when the far end said stop). Lets a consumer show a
  // non-blocking "Reconnecting…" cue. Fired for a PARKED socket too: the
  // connection really is down and the pane really is covered; what parking
  // changes is when the next attempt happens, not whether one is coming.
  onReconnecting: () => void = () => {}

  constructor(url: string, policy: Partial<ReconnectPolicy> = {}) {
    this.url = url
    this.policy = {
      parkWhileHidden: policy.parkWhileHidden ?? false,
      canRetry: policy.canRetry ?? (() => true),
      backoffCapMs: policy.backoffCapMs ?? reconnectBackoffCapMs,
    }
  }

  // A deliberate, user-initiated (re)entry: reset the reconnect bookkeeping
  // (backoff + closedByUser + the stop flag) so a fresh connect never inherits a
  // grown delay or a give-up from a prior session. This is also the manual
  // "Reconnect" path and the take-over bounce.
  connect(): void {
    if (this.disposed) {
      // Debug rather than a warning: a late `connect()` on a disposed socket is
      // an ordering artefact of an unmount, not a fault the user can act on.
      console.debug("[dux] ignoring connect() on a disposed socket", this.url)
      return
    }
    this.closedByUser = false
    this.stopped = false
    this.reconnectDelay = RECONNECT_MIN_MS
    this.clearRetryTimer()
    this.attachWakeSignals()
    // Explicit connects obey the same identity gate as automatic retries. A
    // closed gate defers the attach and polls without growing the fresh backoff.
    if (!this.policy.canRetry()) {
      this.armRetryTimer({ grow: false })
      return
    }
    this.open()
  }

  // A page-lifecycle return (`pageshow`, Chromium's `resume`) or one of the four
  // wake signals: attempt NOW, PLAIN, idempotently. It differs from `connect()`
  // in the two ways that matter: it never touches a socket that is live or
  // connecting, and it keeps the grown backoff for the NEXT failure, because a
  // return signal is evidence about the device rather than about the server.
  //
  // It does clear `closedByUser`, because `pagehide` closes both sockets
  // deliberately and this is the other half of that pair.
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
    if (!this.policy.canRetry()) {
      // The gate is shut. Fall back to the ordinary polling retry rather than
      // opening: a return signal is not permission to attach to a server whose
      // identity has not been confirmed.
      //
      // Repeated wake signals leave an armed timer unchanged and never grow the
      // delay while the gate remains shut.
      if (this.reconnectTimer === null) this.armRetryTimer({ grow: false })
      return
    }
    this.clearRetryTimer()
    this.open()
  }

  // Treat the live connection as dead and let the ORDINARY retry path bring it
  // back: close the socket without setting `closedByUser`, so its own `onclose`
  // runs, `onConn("closed")` is emitted and the backoff schedule takes over.
  //
  // The heartbeat's missed answer is the caller. Going through the real close
  // rather than through `connect()` is deliberate on two counts: the reattach is
  // then PLAIN by construction (a `connect()` detaches its orphan silently, so
  // nothing would broadcast the close that retires an armed take-over), and the
  // consumers that react to a drop see the same event they see for a drop the
  // network caused.
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
    // A socket may already be live here: a double connect() (double-click
    // Reconnect firing connect() mid-reconnect) would otherwise overwrite
    // `this.ws` and leave an orphan whose later `onclose` nulls the SHARED
    // `this.ws` field — permanently killing outbound frames (stale changes pane /
    // frozen terminal, no error). Detach the orphan's handlers and close it
    // BEFORE assigning the new socket so it can never run a handler against
    // shared state again.
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
    this.onConn("connecting")
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
      // An open that LASTS means the connection is usable again, so the next
      // drop starts a fresh retry schedule from the floor. One that does not is
      // no evidence at all, and resetting on it pinned the retry gap at the
      // floor forever; see `HEALTHY_SETTLE_MS`.
      this.armHealthySettle()
      this.onSocketOpen()
      this.onConn("open")
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
      // Only the live socket nulls the shared ref and drives reconnect. Without
      // this identity check an orphan's close would null the live `this.ws`,
      // silently dropping every later outbound frame.
      if (this.ws !== ws) return
      this.clearConnectTimer()
      // This open never earned the reset. Retiring the timer here is what keeps
      // the backoff growing across a flapping connection: left armed, it would
      // fire during the very wait it is supposed to be lengthening.
      this.clearSettleTimer()
      this.ws = null
      this.onConn("closed")
      if (this.closedByUser) return
      // The close carries a code. A server may close with an app-specific code to
      // say "do not retry" — e.g. a PTY whose provider failed to launch or has
      // exited, where re-subscribing would just relaunch the doomed provider.
      // shouldReconnect() inspects the code and, for such a terminal close,
      // surfaces the stop state and returns false. Any other close (a transient
      // transport drop, typically code 1006) retries, indefinitely.
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
    // We are about to retry: signal the consumer so it can show a non-blocking
    // "Reconnecting…" state. Fired once per drop (the timer guard above keeps a
    // single retry in flight).
    this.onReconnecting()
    // PARKED. Schedule nothing at all: a hidden page's timer is throttled to
    // roughly one fire a minute and frozen outright after a few, so an armed
    // retry there is not a retry, it is a promise the platform will not keep.
    // One of the four wake signals picks this straight back up.
    if (this.parked()) return
    this.armRetryTimer()
  }

  private parked(): boolean {
    return (
      this.policy.parkWhileHidden &&
      typeof document !== "undefined" &&
      document.visibilityState === "hidden"
    )
  }

  // Arm the next attempt. `grow` says whether this arming SPENDS a doubling of
  // the backoff, and it is false for every arming the gate caused rather than a
  // failure: the backoff measures how badly the far end is answering, and a shut
  // gate is a fact about this client's own bookkeeping. A gate-held socket
  // therefore polls at a steady interval instead of drifting out to the cap
  // while nothing is actually wrong with the network.
  private armRetryTimer({ grow }: { grow: boolean } = { grow: true }): void {
    const delay = this.reconnectDelay
    if (grow) {
      this.reconnectDelay = Math.min(
        this.reconnectDelay * 2,
        this.policy.backoffCapMs(),
      )
    }
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (this.closedByUser || this.stopped) return
      if (this.parked()) return
      // The gate is consulted here, at the last possible moment, rather than at
      // schedule time: whether the server-run identity check has resolved is a
      // fact about NOW, and a retry held by it re-arms rather than ending, so it
      // resumes on its own without needing to be woken.
      if (!this.policy.canRetry()) {
        this.armRetryTimer({ grow: false })
        return
      }
      this.open()
    }, delay)
  }

  // Abandon a socket that has sat in CONNECTING past the deadline and let the
  // ordinary retry path bring it back. The orphan's handlers are detached first,
  // exactly as `open()` does, so a late callback from a socket the platform
  // finally gets round to resolving can never touch shared state.
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
    }, CONNECT_TIMEOUT_MS)
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

  // The four wake signals. Attached on the first `connect()` and detached only
  // by `dispose()`, so a socket the app really tore down (a pane unmounting) can
  // never be revived by a window event, while one merely CLOSED by the page
  // lifecycle still hears the return that follows.
  private attachWakeSignals(): void {
    if (this.wakeAttached) return
    // Guard on the METHODS rather than on the globals: this runs off-browser
    // (where neither exists) and under test harnesses that stub a partial
    // `document`, and a socket that cannot listen must simply not listen rather
    // than throw on its way to opening.
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

  // A LIFECYCLE CLOSE: the socket goes down deliberately and stays down until
  // something says otherwise, but this object is still in use and the page it
  // belongs to is still there. `pagehide` is the caller that matters, and the
  // wake signals are what bring the socket back afterwards, so they STAY
  // ATTACHED. Detaching them here left a page that returned through anything
  // other than `pageshow` (an unlock reported as `visibilitychange`, a refocus,
  // a network coming back) with two dead sockets and no way back but the
  // Reconnect button.
  close(): void {
    this.closedByUser = true
    this.clearRetryTimer()
    this.clearConnectTimer()
    this.clearSettleTimer()
    this.ws?.close()
  }

  // THE REAL TEARDOWN: this socket will never be used again (its pane
  // unmounted, or switched to a different target). Everything `close()` does,
  // plus the wake listeners, which is the difference between the two and the
  // whole reason both exist.
  dispose(): void {
    this.disposed = true
    this.stopped = true
    this.detachWakeSignals()
    this.close()
  }

  // Reset the backoff. Call it when the connection is confirmed usable so the next
  // drop starts a fresh retry schedule from the minimum delay. The base calls
  // this from `onopen` for sockets whose open proves usability; a subclass whose
  // open does not calls it directly from its own readiness signal instead.
  protected markHealthy(): void {
    this.clearSettleTimer()
    this.reconnectDelay = RECONNECT_MIN_MS
  }

  // ---- Subclass extension hooks ----------------------------------------------

  // Tweak the freshly-constructed WebSocket before handlers are attached (e.g.
  // PtySocket sets `binaryType = "arraybuffer"`). Default: no-op. (`void ws` keeps
  // the param in the base signature — the base calls this with the new socket —
  // without tripping no-unused-vars.)
  protected configureSocket(ws: WebSocket): void {
    void ws
  }

  // Run subclass-specific work on every (re)open, BEFORE `onOpen` fires:
  // EventsSocket re-sends its chunked subscription set here; PtySocket has
  // nothing to resend (the server replays scrollback as the first frame).
  protected abstract onSocketOpen(): void

  // Handle one server frame. EventsSocket parses text; PtySocket splits binary
  // (PTY bytes) from the text control frames.
  protected abstract handleMessage(event: MessageEvent): void

  // Consulted on every unexpected close, before scheduling a reconnect, with the
  // close code. Returning `false` stops the loop for good (PtySocket uses it for a
  // deleted extra tab's now-gone route and for a server "provider unavailable"
  // close code). Default: always reconnect, whatever the code.
  protected shouldReconnect(closeCode: number): boolean {
    void closeCode
    return true
  }

  // React to the socket's `error` event. Default: no-op (EventsSocket); PtySocket
  // logs a breadcrumb. (`void event` keeps the param without tripping the
  // unused-vars lint.)
  protected handleError(event: Event): void {
    void event
  }
}
