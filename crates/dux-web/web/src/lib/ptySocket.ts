// A dedicated WebSocket to one PTY (an agent's provider tab or a companion
// terminal). The connection is the subscription, so the server routes that PTY's
// bytes here with no per-message addressing.
//
// Protocol (matches `handle_pty_socket` in `crates/dux-web/src/server.rs`):
//   - On (re)open the server sends a Text `connected` frame first:
//     `{"event":"connected","id":"<connId>","gen":<n>,"owner":"<connId>"|null,"owner_epoch":<n>,"owner_device":"<ua>","rows":<n>|null,"cols":<n>|null,"grid_seq":<n>}`.
//   - Every applied resize broadcasts `{"event":"size","rows":R,"cols":C,"seq":<n>}`
//     to every socket on that PTY. `seq` is stamped server-side in apply order but
//     published after that order is fixed, so this client keeps only the highest
//     seq seen (seeded from `grid_seq`) and drops any older arrival.
//   - Then one Binary frame replaying the buffered scrollback; feed it straight to
//     xterm. `gen` labels it so a replay already applied can be dropped.
//   - server to client Binary = raw PTY bytes; client to server Binary = PTY stdin.
//   - client to server Text = `{"rows":R,"cols":C}`, which claims sizing and input
//     only when the PTY is unowned. Adding `"takeover":true` transfers ownership,
//     and is the only frame this client sends while it knows it is not the owner.
//   - Close = detach.
//
// Reconnect is the shared `ReconnectingSocket` base with two PTY-specific
// policies: a hidden page schedules nothing, because a PTY nobody is looking at
// is worth nothing until they look again; and a retry is held until the
// run-identity check has resolved, never merely until the events socket is open,
// because attaching an agent's pty launches its provider (`serverValidated.ts`).
// Retrying is otherwise indefinite, `failed` means a terminal close code, and
// `close()` is the deliberate teardown that suppresses the reconnect loop.

import { assertNever } from "./assertNever"
import { ReconnectingSocket } from "./reconnectingSocket"
import { appSocketGivenUp } from "./appSocketGiveUp"
import { onServerValidated, serverValidated } from "./serverValidated"
import type { TerminalOwnerRef } from "./terminalOwner"

// The close code the server sends when the provider is not available to attach
// to. It means "do not auto-retry": re-subscribing would relaunch a doomed
// provider, so the client stops and surfaces Reconnect instead of looping. Must
// match `PROVIDER_GONE_CLOSE_CODE` in `crates/dux-web/src/server.rs`; 4001 is in
// the application-private range, so it cannot collide with a protocol code.
export const PROVIDER_UNAVAILABLE_CLOSE = 4001

// Derive the WebSocket scheme from the page protocol: a hardcoded `ws://` would
// be blocked as mixed content under HTTPS. Read at call time, not module load, so
// the URL builders import safely anywhere and tests can stub `location`.
function wsScheme(): string {
  return location.protocol === "https:" ? "wss:" : "ws:"
}

// The agent session's slot-tab PTY socket URL: an alias for
// `tabPtyUrl(sessionId, <that agent's slot tab>)`, which the server resolves.
// Connecting launches or resumes the provider.
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

// A standalone terminal's PTY socket URL, un-nested because it has no owner to
// nest under. The server still refuses an owned terminal at this address, so it
// is not a way around the nested routes' cross-owner checks.
export function standaloneTerminalPtyUrl(terminalId: string): string {
  return `${wsScheme()}//${location.host}/ws/terminals/${encodeURIComponent(
    terminalId,
  )}/pty`
}

// A terminal's PTY socket URL, chosen by its owner. Which route a terminal is
// reachable at is an ownership decision, so this is a switch ending in
// `assertNever`: a new owner kind must say which route it uses.
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

// A tab's own PTY socket URL, nested under its owning session so the server can
// enforce that the tab belongs to that session. The stable address of every tab,
// the session-slot tab included, which `agentPtyUrl` aliases. Connecting launches
// a dormant tab's provider, resuming or not per the server's dynamic decision.
export function tabPtyUrl(sessionId: string, tabId: string): string {
  return `${wsScheme()}//${location.host}/ws/sessions/${encodeURIComponent(
    sessionId,
  )}/tabs/${encodeURIComponent(tabId)}/pty`
}

interface PtyControlFrame {
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

// A partial grid is unusable because its comparison would be against a size
// the server never reported.
function readGrid(frame: PtyControlFrame): { rows: number; cols: number } | null {
  return typeof frame.rows === "number" && typeof frame.cols === "number"
    ? { rows: frame.rows, cols: frame.cols }
    : null
}

export class PtySocket extends ReconnectingSocket {
  // Retired by `dispose()`, NEVER by `close()`. Kept as a field so the
  // subscription dies with the socket rather than with the module.
  private readonly unsubscribeGate: () => void

  constructor(url: string) {
    // No attempt budget of its own. The budget exists so a page in front of an
    // unreachable server says so instead of spinning, and that is the events
    // socket's job to say: a second give-up underneath would need a second way
    // back. What this socket does instead is HOLD while the app socket has
    // stopped trying, which is the gate's other half.
    super(url, {
      parkWhileHidden: true,
      canRetry: () => serverValidated() && !appSocketGivenUp(),
      attemptBudget: () => 0,
    })
    // The gate pushes as well as blocking: a retry held by it re-arms at whatever
    // delay it had reached, and the gate opening is the moment to try.
    this.unsubscribeGate = onServerValidated(() => {
      this.resumeNow()
    })
  }

  // A lifecycle close (`pagehide`) keeps the gate subscription, as it keeps the
  // wake signals: the gate opening is one of the ways a PTY socket comes back.
  // Only a real teardown retires it.
  override dispose(): void {
    this.unsubscribeGate()
    super.dispose()
  }

  private bytesCb: (bytes: Uint8Array) => void = () => {}
  // This socket's server-assigned connection id, from the `connected` frame on
  // every (re)open (a fresh id per open). Null until that frame arrives; the
  // terminal view compares it against each `pty.owner` event's `owner` to decide
  // ownership (see `ptyOwnership.ts`).
  private connId: string | null = null

  // The generation stamped on the scrollback replay that follows the most recent
  // `connected` frame; null until one carrying `gen` arrives. The pane reads it
  // as it applies a replay and drops any generation it has already applied, so a
  // duplicate or late blob cannot stack a second copy of the scrollback.
  private replayGen: number | null = null

  // Who the server says currently drives this PTY, as of the most recent
  // `connected` frame. Three distinct values, and the pane needs all three:
  //   - a connection id: somebody is driving (this client, if it equals `connId`)
  //   - `null`: the key was present and empty, so nobody is driving
  //   - `undefined`: the key was absent, so this server does not answer and the
  //     client falls back to its foreground guess
  // Only the handshake writes it; live changes arrive as `pty.owner` events on
  // the separate events socket.
  private connectedOwner: string | null | undefined = undefined

  // The ownership epoch stamped on the handshake's owner snapshot
  // (`owner_epoch`), drawn from the same counter every `pty.owner` event carries.
  // The handshake and `pty.owner` ride different connections with no ordering
  // between them, so the seed defers to a strictly newer applied epoch: a stale
  // `connected{owner:null}` would otherwise re-seed this client as a phantom
  // owner, and that direction emits no correcting event. `undefined` means the
  // key was absent, from a server that omits `owner` too.
  private connectedOwnerEpoch: number | undefined = undefined

  // The owner's device label on the handshake's owner snapshot
  // (`owner_device`): the raw `User-Agent` the owning connection presented,
  // the same string a `pty.owner` handover carries as `device`. It rides the
  // handshake because a mere attach hears no handover, leaving a watcher with
  // only generic copy on its take-over card. `undefined` when the key is absent
  // (an unowned pty, or an owner with no User-Agent).
  private connectedOwnerDevice: string | undefined = undefined

  // The PTY's grid as of the most recent frame that reported one: the
  // `connected` handshake at attach, then every `size` event after it. Null means
  // nothing is known about the grid, which must never be mistaken for agreement
  // with the local one.
  private ptyGrid: { rows: number; cols: number } | null = null

  // The highest grid seq applied so far: the handshake's `grid_seq` seed, then
  // every accepted `size` event's `seq`. Announcements can reach this client
  // inverted, so a `size` event at or below this mark carries older geometry than
  // `ptyGrid` already holds and is dropped. Null when no seqs are sent, which
  // disables the filter rather than reading "no seq" as "seq zero".
  private lastGridSeq: number | null = null

  // Fired with this socket's connection id, the pty's current owner, and that
  // snapshot's epoch each time the `connected` frame lands, so the terminal view
  // seeds its ownership verdict from the server rather than from a guess.
  onConnected: (
    id: string,
    owner: string | null | undefined,
    ownerEpoch: number | undefined,
    ownerDevice: string | undefined,
  ) => void = () => {}

  // Fired with the PTY's grid every time the wire reports one. `fromHandshake`
  // separates the state a fresh attach is already sized against from a change
  // after the attach, which is what makes a viewer re-attach to heal. A frame
  // carrying no grid reports null rather than a guess.
  onPtyGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake: boolean,
  ) => void = () => {}

  // The server's answer to one of our beats: `{"event":"beat","n":N}`, echoing
  // the number we sent. The heartbeat matches it against what it is waiting for,
  // so an answer to a stale beat can never satisfy a newer deadline.
  onBeat: (n: number) => void = () => {}

  // `onOpen`, `onReconnecting` and `onConn` are inherited from ReconnectingSocket.
  // Input typed while disconnected is dropped by `sendInput`'s readyState guard;
  // the reconnect cues signal that it would be, they are not a buffer.

  // Consulted on every unexpected close, before scheduling a reconnect. `false`
  // means the underlying PTY route is gone for good rather than merely dropped
  // (an extra tab another client deleted), where retrying would spin with no
  // escape. A close carries no HTTP status the client can read, so the consumer
  // checks its own source of truth, such as spine tab membership. Defaults to
  // always retry.
  shouldRetry: () => boolean = () => true

  // Fired once, in place of scheduling a reconnect, the first time `shouldRetry()`
  // says the route is gone. The consumer decides what the UI does next.
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
  // `connected` frame, or null before that frame and when none was sent.
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
    if (event.data instanceof ArrayBuffer) {
      this.bytesCb(new Uint8Array(event.data))
      return
    }
    if (typeof event.data !== "string") return

    try {
      this.handleControlFrame(JSON.parse(event.data) as PtyControlFrame)
    } catch {
      // A malformed control frame is not fatal to the byte stream.
    }
  }

  private handleControlFrame(frame: PtyControlFrame): void {
    if (frame.event === "beat") {
      if (typeof frame.n === "number") this.onBeat(frame.n)
      return
    }
    if (frame.event === "size") {
      this.handleSizeFrame(frame)
      return
    }
    if (frame.event === "connected" && typeof frame.id === "string") {
      this.handleConnectedFrame(frame, frame.id)
    }
  }

  private handleSizeFrame(frame: PtyControlFrame): void {
    const grid = readGrid(frame)
    if (!grid) return

    if (typeof frame.seq === "number") {
      if (this.lastGridSeq !== null && frame.seq <= this.lastGridSeq) return
      this.lastGridSeq = frame.seq
    }
    this.ptyGrid = grid
    this.onPtyGrid(grid, false)
  }

  private handleConnectedFrame(frame: PtyControlFrame, id: string): void {
    this.connId = id
    this.replayGen = typeof frame.gen === "number" ? frame.gen : null
    // A missing owner means the server does not report ownership; null means
    // the server reports that nobody owns the PTY.
    this.connectedOwner = "owner" in frame ? (frame.owner ?? null) : undefined
    this.connectedOwnerEpoch =
      typeof frame.owner_epoch === "number" ? frame.owner_epoch : undefined
    this.connectedOwnerDevice =
      typeof frame.owner_device === "string" ? frame.owner_device : undefined
    this.ptyGrid = readGrid(frame)
    this.lastGridSeq =
      typeof frame.grid_seq === "number" ? frame.grid_seq : null
    this.onConnected(
      id,
      this.connectedOwner,
      this.connectedOwnerEpoch,
      this.connectedOwnerDevice,
    )
    this.onPtyGrid(this.ptyGrid, true)
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

  // Whether a send right now would actually go on the wire. The send methods
  // below drop frames silently when it is not (fine for keystrokes, which are
  // re-typed), so the compose bar's Send checks this first and instead keeps its
  // buffered message and tells the user.
  get isOpen(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN
  }

  // Send PTY stdin as a Binary frame. A copy is sent so the buffer is a plain
  // `ArrayBuffer` (`WebSocket.send` rejects `ArrayBufferLike` under strict lib
  // typings) and the caller's view cannot mutate it in flight.
  sendInput(bytes: Uint8Array): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(bytes.slice().buffer)
    }
  }

  // Send a resize control frame as Text. The server parses `{rows, cols}` (u16)
  // and issues the SIGWINCH; an unchanged size is a kernel no-op server-side.
  //
  // `takeover` requests an ownership transfer, set only by a deliberate press of
  // Take over. Without it the server grants the claim only on an unowned pty and
  // refuses the resize outright when somebody else drives it, which is what stops
  // an ordinary attach or window change from stealing the prompt. The flag is
  // omitted from the JSON when false rather than sent as `false`.
  //
  // Returns whether the frame actually went on the wire. A dropped resize is
  // re-typed by nobody: the caller skips a size it believes is already there, so
  // a frame discarded here must be reported rather than booked as delivered and
  // never re-asserted. A take-over intent is cleared only when this returns true.
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

  // The one periodic client frame: `{"beat":N,"viewed":B}`.
  //
  // `viewed` never claims sizing ownership server-side; it only stamps the
  // engine's engagement window, so an agent the user is watching keeps its
  // attention flag down without keystrokes. The caller decides it through
  // `shouldSendViewed`. `beat` is the liveness half, and a watcher sends it too:
  // the server's own ping is send-only with no pong deadline, so it cannot see
  // the half-open socket a network handoff leaves behind. The server echoes the
  // number back.
  //
  // Returns whether it went on the wire, so the heartbeat does not start a
  // deadline for a frame it never sent.
  sendBeat(n: number, viewed: boolean): boolean {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ beat: n, viewed }))
      return true
    }
    return false
  }
}

// The PTY socket the focused center pane is currently driving, or null when no
// terminal is focused. Macro delivery is client-side, so the store needs a handle
// to the active PTY without reaching into React. `TerminalPane` registers its
// socket on mount and clears it on unmount.
let activePtySocket: PtySocket | null = null

export function setActivePtySocket(s: PtySocket | null): void {
  activePtySocket = s
}

export function getActivePtySocket(): PtySocket | null {
  return activePtySocket
}
