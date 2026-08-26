// A dedicated WebSocket to ONE PTY (an agent's main provider or a companion
// terminal). Each focused terminal opens its own socket whose connection IS
// the subscription, so the
// server routes that PTY's bytes here with no per-message addressing.
//
// Protocol (matches `handle_pty_socket` in `crates/dux-web/src/server.rs`):
//   - On (re)open the server sends a Text `connected` frame FIRST carrying this
//     socket's connection id, the replay generation, who currently drives the
//     PTY, the ownership epoch of that owner snapshot, the owner's device label
//     (its captured `User-Agent`; the key is omitted when there is none to
//     name), the PTY's current grid, and the grid sequence that grid is at
//     least as new as:
//     `{"event":"connected","id":"<connId>","gen":<n>,"owner":"<connId>"|null,"owner_epoch":<n>,"owner_device":"<ua>","rows":<n>|null,"cols":<n>|null,"grid_seq":<n>}`.
//   - Whenever a resize is APPLIED to the PTY, every socket attached to it gets
//     a Text event frame `{"event":"size","rows":R,"cols":C,"seq":<n>}`. One PTY
//     has one authoritative grid (the owner's) and every other attached browser
//     renders the same byte stream into its own differently sized xterm, so this
//     is how a viewer learns that what it is rendering is wrapped and clamped.
//     `seq` is a per-pty counter stamped server-side in APPLY order; the
//     broadcasts behind these frames are emitted after that order is fixed and
//     can invert in flight, so this client keeps only the highest seq seen
//     (seeded from the handshake's `grid_seq`) and drops any older arrival, or
//     a stale announcement could become its last word on the grid.
//   - Then the server sends ONE Binary frame replaying the buffered
//     scrollback/repaint; feed it straight to xterm like any other byte chunk. The
//     `gen` labels this replay so the client can drop one it already applied.
//   - server→client Binary = raw PTY bytes (write to xterm).
//   - client→server Binary = PTY stdin (xterm `onData`).
//   - client→server Text = a resize control frame `{"rows":R,"cols":C}`, which
//     claims sizing+input ONLY when the PTY is unowned. A resize that also
//     carries `"takeover":true` transfers ownership from whoever holds it, and
//     is the only frame this client ever sends while it knows it is not the
//     owner.
//   - Close = detach (the server drops the subscription/forwarder).
//
// Reconnect behavior is the shared `ReconnectingSocket` base, with the two
// PTY-specific policies it takes:
//
//   PARKING. A hidden page schedules nothing. This is the socket the policy
//   exists for: the events socket keeps retrying while hidden, because attention
//   indicators and OS notifications ride it precisely then, while a PTY nobody is
//   looking at is worth nothing until they look again.
//
//   THE VALIDATED GATE. A retry is held until the run-identity check has
//   RESOLVED, never merely until the events socket is open. Attaching an agent's
//   pty LAUNCHES its provider, so attaching to a server that restarted under this
//   tab is the one thing the check exists to prevent, and `conn === "open"` is
//   true a whole round trip before it has answered. See `serverValidated.ts`.
//
// Retrying is otherwise indefinite; `failed` means a terminal close code and
// nothing else. `close()` is the deliberate, user-initiated teardown and
// suppresses the reconnect loop.

import { assertNever } from "./assertNever"
import { ReconnectingSocket } from "./reconnectingSocket"
import { serverValidated } from "./serverValidated"
import type { TerminalOwnerRef } from "./terminalOwner"

// The WebSocket close code the server sends on a PTY socket when the provider is
// not available to attach to — it failed to launch (e.g. the CLI is not on PATH)
// or its process has exited/crashed. It means "do not auto-retry": re-subscribing
// would just relaunch the doomed provider, so the client stops and surfaces the
// Reconnect affordance instead of looping. Must match `PROVIDER_GONE_CLOSE_CODE`
// in `crates/dux-web/src/server.rs`. 4001 is in the application-private range
// (4000-4999) so it can never collide with a protocol close code.
export const PROVIDER_UNAVAILABLE_CLOSE = 4001

// Derive the WebSocket scheme from the page protocol so an HTTPS deployment uses
// `wss://` (a hardcoded `ws://` would be blocked as mixed content under HTTPS).
// Read at call time (not module load) so the URL builders are safe to import in
// any environment and tests can stub `location` per-case.
function wsScheme(): string {
  return location.protocol === "https:" ? "wss:" : "ws:"
}

// The agent session's main PTY socket URL. Connecting launches/resumes the
// provider, exactly as the legacy `Subscribe` did.
export function agentPtyUrl(sessionId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/pty`
}

// A companion terminal's PTY socket URL, nested under its owning session so the
// server can enforce that the terminal belongs to that session.
export function terminalPtyUrl(sessionId: string, terminalId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/terminals/${encodeURIComponent(terminalId)}/pty`
}

// A project terminal's PTY socket URL, nested under its owning project so the
// server can enforce that the terminal belongs to that project.
export function projectTerminalPtyUrl(
  projectId: string,
  terminalId: string,
): string {
  return `${wsScheme()}//${location.host}/ws/projects/${encodeURIComponent(
    projectId,
  )}/terminals/${encodeURIComponent(terminalId)}/pty`
}

// A standalone terminal's PTY socket URL. Un-nested, because a standalone
// terminal has no owner to nest under; the server still refuses an owned
// terminal at this address, so the address is not a way around the nested
// routes' cross-owner checks.
export function standaloneTerminalPtyUrl(terminalId: string): string {
  return `${wsScheme()}//${location.host}/ws/terminals/${encodeURIComponent(
    terminalId,
  )}/pty`
}

// A terminal's PTY socket URL, chosen by its OWNER. Which websocket route a
// terminal is reachable at is an ownership decision, so it is a switch ending in
// `assertNever` rather than a two-way conditional at the call site: a new owner
// kind is reachable at a route of its own, and must say which.
export function terminalSocketUrl(
  owner: TerminalOwnerRef,
  terminalId: string,
): string {
  switch (owner.kind) {
    case "session":
      return terminalPtyUrl(owner.sessionId, terminalId)
    case "project":
      return projectTerminalPtyUrl(owner.projectId, terminalId)
    case "standalone":
      return standaloneTerminalPtyUrl(terminalId)
    default:
      return assertNever(owner)
  }
}

// An extra tab's PTY socket URL, nested under its owning session so the server
// can enforce that the tab belongs to that session. Used ONLY for extra tabs;
// the session-slot tab keeps `agentPtyUrl` (its `tab_id === session_id`, served by
// the existing `/ws/sessions/:id/pty` route). Connecting launches the extra
// tab's provider fresh (there is no resume for extra tabs).
export function tabPtyUrl(sessionId: string, tabId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/tabs/${encodeURIComponent(tabId)}/pty`
}

// A grid off a wire frame, or null when the frame does not carry one. Both
// dimensions must be real numbers: half a grid is not a grid, and a partial
// read would be worse than none, because the comparison it feeds would then be
// against a number nobody sent.
function readGrid(frame: {
  rows?: number | null
  cols?: number | null
}): { rows: number; cols: number } | null {
  return typeof frame.rows === "number" && typeof frame.cols === "number"
    ? { rows: frame.rows, cols: frame.cols }
    : null
}

export class PtySocket extends ReconnectingSocket {
  constructor(url: string) {
    super(url, { parkWhileHidden: true, canRetry: serverValidated })
  }

  private bytesCb: (bytes: Uint8Array) => void = () => {}
  // This socket's server-assigned connection id, delivered as the first Text frame
  // (`{event:"connected", id}`) on every (re)open (the server allocates a fresh id
  // per open). Null until that frame arrives. The terminal view compares it against
  // the `owner` field of each `pty.owner` event to decide ownership definitively
  // (see `ptyOwnership.ts`).
  private connId: string | null = null

  // The generation stamped on the scrollback replay that follows the most recent
  // `connected` frame (see the reconnect-repaint idempotency guard in
  // `TerminalPane`). Null until a `connected` frame carrying `gen` arrives, or when
  // an older server sends none. The pane reads this the instant it applies a replay
  // and drops any replay whose generation it has already applied, so a duplicate or
  // late blob can never stack a second copy of the scrollback.
  private replayGen: number | null = null

  // Who the server says currently drives this PTY, as of the most recent
  // `connected` frame. THREE distinct values, and the pane needs all three:
  //   - a connection id: somebody is driving (this client, if it equals `connId`)
  //   - `null`: the key was present and empty, so nobody is driving
  //   - `undefined`: the key was ABSENT, so this server does not answer the
  //     question and the client falls back to its foreground guess
  // Only the handshake writes it; live changes arrive as `pty.owner` events on
  // the separate events socket.
  private connectedOwner: string | null | undefined = undefined

  // The ownership epoch stamped on the handshake's owner snapshot
  // (`owner_epoch`), read server-side under the same lock as `owner` and drawn
  // from the SAME counter every `pty.owner` event carries. The handshake rides
  // this socket while `pty.owner` rides the events socket, two TCP connections
  // with no ordering between them, so the seed compares this against the newest
  // `pty.owner` epoch already applied and DEFERS to a strictly newer one:
  // without it, a stale `connected{owner:null}` arriving after a fresh
  // `pty.owner{owner:B}` would re-seed this client as a phantom owner, and the
  // stale-null direction emits no correcting event, ever. `undefined` means the
  // key was absent (an old server, which then omitted `owner` too).
  private connectedOwnerEpoch: number | undefined = undefined

  // The owner's device label on the handshake's owner snapshot
  // (`owner_device`): the raw `User-Agent` the owning connection presented at
  // its upgrade, recorded server-side at claim time and read under the same
  // lock as `owner`. It is the same string a `pty.owner` handover carries as
  // `device`, and it rides the handshake because a mere attach hears no such
  // handover: without it a watcher that simply opened the pane could only
  // title its take-over card with the generic copy. `undefined` when the key
  // is absent (an old server, an unowned pty, or an owner with no User-Agent).
  private connectedOwnerDevice: string | undefined = undefined

  // The PTY's grid as of the most recent frame that reported one: the
  // `connected` handshake at attach, then every `size` event after it. Null
  // when the server did not answer (an old server, or a pty it could not read),
  // which reads as "nothing is known about the grid" and must never be mistaken
  // for agreement with the local one.
  private ptyGrid: { rows: number; cols: number } | null = null

  // The highest grid seq applied so far: the handshake's `grid_seq` seed, then
  // every accepted `size` event's `seq`. The server stamps seqs in apply order
  // but publishes after that order is fixed, so two sockets' announcements can
  // reach this client inverted; a `size` event at or below this mark carries
  // OLDER geometry than `ptyGrid` already holds and is dropped, never applied.
  // Null against an old server that sends no seqs, which disables the filter
  // rather than mistaking "no seq" for "seq zero".
  private lastGridSeq: number | null = null

  // Fired with this socket's connection id, the pty's current owner, and the
  // ownership epoch of that owner snapshot, each time the `connected` frame
  // lands. Lets the terminal view track which connection id is "us" for the
  // ownership comparison, and SEED its verdict from the server rather than from
  // a guess, re-issued on every reconnect.
  onConnected: (
    id: string,
    owner: string | null | undefined,
    ownerEpoch: number | undefined,
    ownerDevice: string | undefined,
  ) => void = () => {}

  // Fired with the PTY's grid every time the wire reports one, and with
  // `fromHandshake` saying WHICH frame reported it. The two are acted on
  // differently and the distinction is the whole point of the flag: the
  // handshake's grid is the state a fresh attach is already sized against,
  // while a CHANGE after the attach is what makes a viewer re-attach to heal.
  // A frame that carries no grid (an old server, or a pty the server could not
  // read) reports null rather than a guess.
  onPtyGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake: boolean,
  ) => void = () => {}

  // The server's answer to one of our beats: `{"event":"beat","n":N}`, echoing
  // the number we sent. The heartbeat matches it against what it is waiting for,
  // so an answer to a stale beat can never satisfy a newer deadline.
  onBeat: (n: number) => void = () => {}

  // `onOpen`, `onReconnecting`, and `onConn` are inherited from ReconnectingSocket.
  // The pane wires `onOpen` (re-arm first-frame resize; the server replays
  // scrollback as the first Binary frame on every open), `onReconnecting` (show a
  // non-blocking "Reconnecting…" cue while the socket retries), and `onConn` (so
  // it can surface a "connection lost" Reconnect affordance when the shared cap is
  // hit and the socket emits `failed`). Input typed while disconnected is still
  // dropped by `sendInput`'s readyState guard — the cues signal that it would be,
  // they are not a buffer.

  // Consulted on every unexpected close, BEFORE scheduling a reconnect. Returning
  // `false` means the underlying PTY route is gone for good (e.g. an extra tab's
  // socket 404s because another client deleted that tab while this one was
  // retrying) rather than merely dropped — retrying against a route that will keep
  // 404ing would spin with no escape. A close carries no HTTP status the client
  // can read, so the consumer is expected to check its own source of truth (e.g.
  // spine tab membership) instead. Defaults to always retry, matching every other
  // PTY socket (agent session-slot tab, companion terminal), which never go away
  // out from under a live client this way.
  shouldRetry: () => boolean = () => true

  // Fired once, in place of scheduling a reconnect, the first time `shouldRetry()`
  // says the route is gone. The socket does not close itself further (there is
  // nothing more to tear down); the consumer decides what the UI does next.
  onGone: () => void = () => {}

  // Register the raw-bytes consumer (xterm `term.write`). Last registration wins.
  onBytes(cb: (bytes: Uint8Array) => void): void {
    this.bytesCb = cb
  }

  // This socket's current connection id, or null before the `connected` frame.
  get connectionId(): string | null {
    return this.connId
  }

  // The generation of the scrollback replay that immediately follows the current
  // `connected` frame, or null when the server sent none (an older server) or before
  // the first `connected` frame. Read by the pane at the moment it applies a replay.
  get replayGeneration(): number | null {
    return this.replayGen
  }

  // The pty's owner as of the most recent `connected` frame; see the field.
  get handshakeOwner(): string | null | undefined {
    return this.connectedOwner
  }

  // The PTY's grid as last reported by the wire, or null when nothing has
  // reported one; see the field.
  get grid(): { rows: number; cols: number } | null {
    return this.ptyGrid
  }

  // Request arraybuffer framing so server→client Binary frames arrive as
  // `ArrayBuffer` (raw PTY bytes) rather than Blobs.
  protected configureSocket(ws: WebSocket): void {
    ws.binaryType = "arraybuffer"
  }

  // Nothing to resend on (re)open: the server replays this PTY's scrollback as the
  // first Binary frame after every open, so the byte feed rehydrates itself.
  protected onSocketOpen(): void {}

  protected handleMessage(event: MessageEvent): void {
    // Binary frames carry PTY bytes (the scrollback replay arrives as an ordinary
    // Binary frame too). The ONLY Text frame the server sends is the opening
    // `connected` handshake carrying this socket's connection id; record it for
    // the ownership comparison and notify the consumer.
    if (event.data instanceof ArrayBuffer) {
      this.bytesCb(new Uint8Array(event.data))
      return
    }
    if (typeof event.data === "string") {
      try {
        const frame = JSON.parse(event.data) as {
          event?: string
          n?: number
          id?: string
          gen?: number
          owner?: string | null
          owner_epoch?: number
          owner_device?: string
          rows?: number | null
          cols?: number | null
          seq?: number
          grid_seq?: number
        }
        // The answer to one of our beats. Handled first because it is by far the
        // most frequent text frame on a quiet socket.
        if (frame.event === "beat") {
          if (typeof frame.n === "number") this.onBeat(frame.n)
          return
        }
        // A grid change on a PTY somebody else is driving. Handled before the
        // handshake branch because the two are told apart by `event` alone.
        if (frame.event === "size") {
          const grid = readGrid(frame)
          if (grid) {
            // Drop a stale announcement: one whose seq is at or below the
            // newest applied (the handshake's seed included, so a broadcast
            // buffered from before the handshake cannot regress the grid the
            // handshake just reported). An event with no seq is an old
            // server, which never stamped an order to enforce.
            if (typeof frame.seq === "number") {
              if (this.lastGridSeq !== null && frame.seq <= this.lastGridSeq) {
                return
              }
              this.lastGridSeq = frame.seq
            }
            this.ptyGrid = grid
            this.onPtyGrid(grid, false)
          }
          return
        }
        if (frame.event === "connected" && typeof frame.id === "string") {
          this.connId = frame.id
          // Record the replay generation for the Binary frame that follows. A frame
          // without `gen` (older server) leaves it null, which the guard reads as
          // "always apply" so the change is backward-safe.
          this.replayGen = typeof frame.gen === "number" ? frame.gen : null
          // ABSENT and NULL are different answers here, so this reads the KEY,
          // not the value: `owner: null` means "nobody is driving, claim it if
          // you are foregrounded", while a missing key means "this server does
          // not say" and the client must not read that as unowned.
          this.connectedOwner =
            "owner" in frame ? (frame.owner ?? null) : undefined
          // The epoch travels with `owner`: a server that answers the owner
          // question stamps the snapshot, and an old server omits both keys
          // together, so an absent epoch is the same mixed-version signal.
          this.connectedOwnerEpoch =
            typeof frame.owner_epoch === "number" ? frame.owner_epoch : undefined
          // The owner's device label. Absent (not null) whenever there is no
          // name to give, so a bare presence check is enough; only a string is
          // ever accepted.
          this.connectedOwnerDevice =
            typeof frame.owner_device === "string"
              ? frame.owner_device
              : undefined
          // The grid this attach is joining. A server that cannot answer sends
          // explicit nulls (and an older one omits the keys), and both land
          // here as null: "nothing known", never "it matches".
          this.ptyGrid = readGrid(frame)
          // Seed the seq filter from the handshake: the server read `grid_seq`
          // before the grid, so the grid above is at least as new as this
          // mark, and any `size` event at or below it is stale. Absent on an
          // old server, which leaves the filter off.
          this.lastGridSeq =
            typeof frame.grid_seq === "number" ? frame.grid_seq : null
          this.onConnected(
            frame.id,
            this.connectedOwner,
            this.connectedOwnerEpoch,
            this.connectedOwnerDevice,
          )
          this.onPtyGrid(this.ptyGrid, true)
        }
      } catch {
        // A malformed control frame is not fatal to the byte stream; ignore it.
      }
    }
  }

  // A hard stop, not a transient drop, in two cases; otherwise reconnect normally
  // (up to the shared attempt cap).
  protected shouldReconnect(closeCode: number): boolean {
    // 1. The server closed with the provider-unavailable code: the provider
    //    failed to launch or has exited, so re-subscribing would only relaunch a
    //    doomed provider. Surface the same stop state as hitting the cap (the pane
    //    shows Reconnect; a manual reconnect resets the budget) and do not retry.
    if (closeCode === PROVIDER_UNAVAILABLE_CLOSE) {
      this.onConn("failed")
      return false
    }
    // 2. The underlying route is gone (e.g. an extra tab another client deleted
    //    while this one was retrying): fire `onGone` once and stop.
    if (!this.shouldRetry()) {
      this.onGone()
      return false
    }
    return true
  }

  // Warn so a flapping PTY socket leaves a console breadcrumb instead of failing
  // silently; the visible reconnect signal is driven by `onReconnecting`.
  protected handleError(event: Event): void {
    console.warn("[dux] PTY socket error; reconnect will follow", event)
  }

  // Whether the underlying WebSocket is currently open, i.e. whether a send
  // right now would actually go on the wire. The send methods below silently
  // drop frames when it is not (fine for keystrokes, which are re-typed), but
  // the compose bar's Send must instead KEEP its buffered message and tell the
  // user, so it checks this before writing.
  get isOpen(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN
  }

  // Send PTY stdin as a Binary frame. A copy is sent so the buffer is a plain
  // `ArrayBuffer` (not `ArrayBufferLike`, which `WebSocket.send` rejects under
  // strict lib typings) and the caller's view can't mutate it in flight.
  sendInput(bytes: Uint8Array): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(bytes.slice().buffer)
    }
  }

  // Send a resize control frame as Text. The server parses `{rows, cols}` (u16)
  // and issues the SIGWINCH; an unchanged size is a kernel no-op server-side.
  //
  // `takeover` is the ownership TRANSFER request, set only by a deliberate press
  // of Take over. Without it the server grants the claim only when the pty is
  // unowned, and refuses the resize outright when somebody else drives it, which
  // is what stops an ordinary attach or window change from stealing the prompt.
  // The flag is omitted from the JSON when false rather than sent as `false`, so
  // the ordinary frame is byte-identical to the one every prior version sent.
  //
  // Returns whether the frame actually went on the wire. Unlike a keystroke, a
  // dropped resize is not re-typed by anybody: the caller remembers the last
  // size it told the PTY about and skips a size it believes is already there,
  // so a frame silently discarded here (the socket is CONNECTING or CLOSED,
  // which is every reconnect) must be reported rather than swallowed, or the
  // size is booked as delivered and never re-asserted. A take-over intent rides
  // on exactly this answer: it is cleared only when this returns true.
  sendResize(
    rows: number,
    cols: number,
    takeover = false,
    expectedOwner?: string,
  ): boolean {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      const frame = takeover
        ? expectedOwner === undefined
          ? { rows, cols, takeover: true }
          : { rows, cols, takeover: true, expected_owner: expectedOwner }
        : { rows, cols }
      this.ws.send(JSON.stringify(frame))
      return true
    }
    return false
  }

  // THE ONE PERIODIC CLIENT FRAME: `{"beat":N,"viewed":B}`.
  //
  // `viewed` is the older half. It NEVER claims sizing ownership server-side; it
  // only stamps the engine's engagement window, so an agent the user is actively
  // watching keeps its attention flag down without requiring keystrokes. The
  // caller decides it through `shouldSendViewed`.
  //
  // `beat` is the liveness half, and a WATCHER sends it too (with `viewed`
  // false): the server's own WebSocket ping is send-only with no pong deadline,
  // so it reaps a socket the OS has given up on but cannot see the half-open one
  // a Wi-Fi to cellular handoff leaves behind. The server echoes the number back.
  //
  // One frame rather than two, because they run on the same timer and a second
  // periodic frame is a second thing to keep in step. Returns whether it went on
  // the wire, so the heartbeat does not start a deadline for a frame it never
  // sent.
  sendBeat(n: number, viewed: boolean): boolean {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ beat: n, viewed }))
      return true
    }
    return false
  }
}

// The PTY socket the focused center pane is currently driving, or null when no
// terminal is focused. The macro quick-picker writes a macro's payload straight
// to this socket as stdin (there is no server-side `run_macro` command;
// delivery is client-side),
// so the store needs a handle to "the active PTY" without reaching into React.
// `TerminalPane` registers its socket on mount and clears it on unmount.
let activePtySocket: PtySocket | null = null

export function setActivePtySocket(s: PtySocket | null): void {
  activePtySocket = s
}

export function getActivePtySocket(): PtySocket | null {
  return activePtySocket
}
