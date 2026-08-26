import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  PROVIDER_UNAVAILABLE_CLOSE,
  PtySocket,
  agentPtyUrl,
  getActivePtySocket,
  setActivePtySocket,
  projectTerminalPtyUrl,
  standaloneTerminalPtyUrl,
  tabPtyUrl,
  terminalPtyUrl,
  terminalSocketUrl,
} from "./ptySocket"
import { clearServerValidated, noteServerValidated } from "./serverValidated"
import type { ConnState } from "./types"

// A controllable WebSocket double: tests trigger open/message/close explicitly
// and inspect the frames the socket sent. `OPEN` is static so the socket's
// `readyState === WebSocket.OPEN` send guard resolves. `binaryType` is recorded
// so we can assert the socket asks for arraybuffer framing.
class FakeWS {
  static OPEN = 1
  static instances: FakeWS[] = []
  url: string
  binaryType = ""
  readyState = 0
  // Sent frames: strings (resize) and ArrayBuffers (stdin), in order.
  sent: (string | ArrayBuffer)[] = []
  onopen: (() => void) | null = null
  onclose: ((e: { code: number }) => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((e: { data: string | ArrayBuffer }) => void) | null = null

  constructor(url: string) {
    this.url = url
    FakeWS.instances.push(this)
  }

  send(data: string | ArrayBuffer): void {
    this.sent.push(data)
  }

  close(code = 1000): void {
    this.readyState = 3
    this.onclose?.({ code })
  }

  // Drive the lifecycle from the test.
  open(): void {
    this.readyState = 1
    this.onopen?.()
  }

  // Deliver a server→client Binary frame (raw PTY bytes).
  binary(bytes: number[]): void {
    this.onmessage?.({ data: new Uint8Array(bytes).buffer })
  }

  // Deliver a server→client Text frame (the `connected` handshake).
  text(payload: string): void {
    this.onmessage?.({ data: payload })
  }

  // Close with a code; default 1006 (abnormal) models a transient transport drop.
  triggerClose(code = 1006): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
}

beforeEach(() => {
  FakeWS.instances = []
  vi.stubGlobal("WebSocket", FakeWS)
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7070" })
  // The retry gate is real (see `serverValidated.ts`): a PTY socket may not
  // attach until the run-identity check has resolved. Every case here is about
  // something else, so the gate is opened once, centrally, and the one test that
  // is about the gate shuts it itself.
  noteServerValidated()
})

afterEach(() => {
  clearServerValidated()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  setActivePtySocket(null)
})

function last(): FakeWS {
  const ws = FakeWS.instances.at(-1)
  if (!ws) throw new Error("no socket constructed")
  return ws
}

describe("ptySocket URL builders", () => {
  it("builds the agent PTY URL (ws under http)", () => {
    expect(agentPtyUrl("s1")).toBe("ws://localhost:7070/ws/sessions/s1/pty")
  })

  it("builds the companion-terminal PTY URL nested under its session", () => {
    expect(terminalPtyUrl("s1", "t9")).toBe(
      "ws://localhost:7070/ws/sessions/s1/terminals/t9/pty",
    )
  })

  it("builds the project-terminal PTY URL nested under its project", () => {
    // A typo'd path here would 404 forever through the reconnecting socket
    // with no visible error, so pin the exact string.
    expect(projectTerminalPtyUrl("p1", "t9")).toBe(
      "ws://localhost:7070/ws/projects/p1/terminals/t9/pty",
    )
  })

  it("builds the standalone-terminal PTY URL un-nested, naming no owner", () => {
    expect(standaloneTerminalPtyUrl("t9")).toBe(
      "ws://localhost:7070/ws/terminals/t9/pty",
    )
  })

  it("routes each owner kind to its own socket", () => {
    // Which address a terminal is reachable at is an ownership decision, so pin
    // all three: a terminal sent to the wrong route 404s forever behind the
    // reconnecting socket, with nothing visible saying why.
    expect(terminalSocketUrl({ kind: "session", sessionId: "s1" }, "t9")).toBe(
      "ws://localhost:7070/ws/sessions/s1/terminals/t9/pty",
    )
    expect(terminalSocketUrl({ kind: "project", projectId: "p1" }, "t9")).toBe(
      "ws://localhost:7070/ws/projects/p1/terminals/t9/pty",
    )
    expect(terminalSocketUrl({ kind: "standalone" }, "t9")).toBe(
      "ws://localhost:7070/ws/terminals/t9/pty",
    )
  })

  it("builds the extra-tab PTY URL nested under its session", () => {
    expect(tabPtyUrl("s1", "tab9")).toBe(
      "ws://localhost:7070/ws/sessions/s1/tabs/tab9/pty",
    )
  })

  it("encodes ids and uses wss under https", () => {
    vi.stubGlobal("location", { protocol: "https:", host: "example.com" })
    expect(agentPtyUrl("a b")).toBe("wss://example.com/ws/sessions/a%20b/pty")
    expect(terminalPtyUrl("s/1", "t/2")).toBe(
      "wss://example.com/ws/sessions/s%2F1/terminals/t%2F2/pty",
    )
    expect(tabPtyUrl("s/1", "b/2")).toBe(
      "wss://example.com/ws/sessions/s%2F1/tabs/b%2F2/pty",
    )
    expect(projectTerminalPtyUrl("p/1", "t/2")).toBe(
      "wss://example.com/ws/projects/p%2F1/terminals/t%2F2/pty",
    )
    expect(standaloneTerminalPtyUrl("t/2")).toBe(
      "wss://example.com/ws/terminals/t%2F2/pty",
    )
  })
})

describe("PtySocket", () => {
  it("connects to the given URL and requests arraybuffer framing", () => {
    const sock = new PtySocket("ws://x/ws/sessions/s1/pty")
    sock.connect()
    const ws = last()
    expect(ws.url).toBe("ws://x/ws/sessions/s1/pty")
    expect(ws.binaryType).toBe("arraybuffer")
  })

  it("records connection id and replay generation from the connected frame", () => {
    const sock = new PtySocket("ws://x/pty")
    let connectedId: string | null = null
    sock.onConnected = (id) => {
      connectedId = id
    }
    sock.connect()
    last().open()
    expect(sock.connectionId).toBeNull()
    expect(sock.replayGeneration).toBeNull()
    last().text(JSON.stringify({ event: "connected", id: "c-1", gen: 42 }))
    expect(connectedId).toBe("c-1")
    expect(sock.connectionId).toBe("c-1")
    expect(sock.replayGeneration).toBe(42)
  })

  it("leaves replayGeneration null when the connected frame omits gen", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().text(JSON.stringify({ event: "connected", id: "c-2" }))
    expect(sock.connectionId).toBe("c-2")
    expect(sock.replayGeneration).toBeNull()
  })

  // ONE PTY HAS ONE AUTHORITATIVE GRID, the owner's, and every other attached
  // browser renders the same byte stream into its own differently sized xterm.
  // The wire is the only way a viewer can learn that, and it learns it twice
  // over: the handshake's snapshot at attach, and a `size` event on every
  // applied resize after it. The `fromHandshake` flag is what tells them apart,
  // and the consumer acts on them differently (an attach is already sized
  // against the handshake; only a later CHANGE makes a viewer re-attach).
  it("reports the pty's grid from the handshake and from every size event", () => {
    const sock = new PtySocket("ws://x/pty")
    const seen: [{ rows: number; cols: number } | null, boolean][] = []
    sock.onPtyGrid = (grid, fromHandshake) => {
      seen.push([grid, fromHandshake])
    }
    sock.connect()
    last().open()
    expect(sock.grid).toBeNull()
    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-1",
        gen: 1,
        rows: 24,
        cols: 80,
      }),
    )
    expect(sock.grid).toEqual({ rows: 24, cols: 80 })
    last().text(JSON.stringify({ event: "size", rows: 40, cols: 120 }))
    expect(sock.grid).toEqual({ rows: 40, cols: 120 })
    expect(seen).toEqual([
      [{ rows: 24, cols: 80 }, true],
      [{ rows: 40, cols: 120 }, false],
    ])
  })

  it("reads a grid the server could not answer as UNKNOWN, never as agreement", () => {
    // A server that cannot read the pty sends explicit nulls, and an older one
    // omits the keys. Both mean "nothing known": a viewer that read either as
    // "it matches mine" would sit silently on wrapped, clamped output.
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-1",
        gen: 1,
        rows: null,
        cols: null,
      }),
    )
    expect(sock.grid).toBeNull()
    last().text(JSON.stringify({ event: "connected", id: "c-2", gen: 2 }))
    expect(sock.grid).toBeNull()
  })

  it("drops a size event whose seq the handshake already covers", () => {
    // The server stamps seqs in apply order but publishes after that order is
    // fixed, so a broadcast can arrive AFTER a handshake whose grid already
    // reflects it (it was buffered on the socket when the handshake was sent).
    // Applying it would regress the grid to an older geometry and, worse, read
    // as a fresh change and arm a heal bounce at a just-attached socket.
    const sock = new PtySocket("ws://x/pty")
    const seen: [{ rows: number; cols: number } | null, boolean][] = []
    sock.onPtyGrid = (grid, fromHandshake) => {
      seen.push([grid, fromHandshake])
    }
    sock.connect()
    last().open()
    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-1",
        gen: 1,
        rows: 40,
        cols: 120,
        grid_seq: 5,
      }),
    )
    // Stale: seq 4 predates the handshake's read. Neither the grid nor the
    // consumer hears it, so nothing downstream can arm a heal from it.
    last().text(JSON.stringify({ event: "size", rows: 24, cols: 80, seq: 4 }))
    expect(sock.grid).toEqual({ rows: 40, cols: 120 })
    // A seq equal to the handshake's is covered too.
    last().text(JSON.stringify({ event: "size", rows: 24, cols: 80, seq: 5 }))
    expect(sock.grid).toEqual({ rows: 40, cols: 120 })
    // A genuinely newer change still lands and still reads as a change.
    last().text(JSON.stringify({ event: "size", rows: 30, cols: 100, seq: 6 }))
    expect(sock.grid).toEqual({ rows: 30, cols: 100 })
    expect(seen).toEqual([
      [{ rows: 40, cols: 120 }, true],
      [{ rows: 30, cols: 100 }, false],
    ])
  })

  it("drops a size event that arrives behind a newer one, by seq alone", () => {
    // Two sockets' publishes of two ORDERED applies can invert on the way to
    // this client (the take-over interleaving): the newer geometry lands
    // first, then the stale one. Last-write-wins without the seq would leave
    // every viewer on the loser's geometry.
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-1",
        gen: 1,
        rows: 24,
        cols: 80,
        grid_seq: 1,
      }),
    )
    last().text(JSON.stringify({ event: "size", rows: 30, cols: 100, seq: 3 }))
    last().text(JSON.stringify({ event: "size", rows: 26, cols: 90, seq: 2 }))
    expect(sock.grid).toEqual({ rows: 30, cols: 100 })
  })

  it("applies size events without a seq, for an old server that stamps none", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().text(
      JSON.stringify({ event: "connected", id: "c-1", gen: 1, rows: 24, cols: 80 }),
    )
    last().text(JSON.stringify({ event: "size", rows: 30, cols: 100 }))
    expect(sock.grid).toEqual({ rows: 30, cols: 100 })
  })

  // THREE distinct answers to "who drives this pty", and the pane needs all
  // three: a null OWNER means "nobody, claim it if you are foregrounded", while
  // an ABSENT owner key means "this server does not say" and must fall back to
  // the foreground guess. Collapsing the two would make an old server look like
  // an unowned pty on every attach, which is the silent steal reborn.
  it("tells an absent owner key apart from an explicitly null one", () => {
    const seen: (string | null | undefined)[] = []
    const sock = new PtySocket("ws://x/pty")
    sock.onConnected = (_id, owner) => {
      seen.push(owner)
    }
    sock.connect()
    last().open()

    last().text(JSON.stringify({ event: "connected", id: "c-1", gen: 1 }))
    expect(seen.at(-1)).toBeUndefined()
    expect(sock.handshakeOwner).toBeUndefined()

    last().text(
      JSON.stringify({ event: "connected", id: "c-2", gen: 2, owner: null }),
    )
    expect(seen.at(-1)).toBeNull()
    expect(sock.handshakeOwner).toBeNull()

    last().text(
      JSON.stringify({ event: "connected", id: "c-3", gen: 3, owner: "c-9" }),
    )
    expect(seen.at(-1)).toBe("c-9")
    expect(sock.handshakeOwner).toBe("c-9")
  })

  // The epoch stamps the handshake's owner snapshot so the seed can defer to a
  // strictly newer `pty.owner` that arrived on the OTHER socket first. An old
  // server omits it together with `owner`, and the callback then reports
  // undefined so the mixed-version fallback stays intact.
  it("passes the handshake's owner_epoch through, and undefined when absent", () => {
    const epochs: (number | undefined)[] = []
    const sock = new PtySocket("ws://x/pty")
    sock.onConnected = (_id, _owner, ownerEpoch) => {
      epochs.push(ownerEpoch)
    }
    sock.connect()
    last().open()

    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-1",
        gen: 1,
        owner: null,
        owner_epoch: 0,
      }),
    )
    expect(epochs.at(-1)).toBe(0)

    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-2",
        gen: 2,
        owner: "c-9",
        owner_epoch: 7,
      }),
    )
    expect(epochs.at(-1)).toBe(7)

    // Old server: no owner key, no epoch.
    last().text(JSON.stringify({ event: "connected", id: "c-3", gen: 3 }))
    expect(epochs.at(-1)).toBeUndefined()
  })

  // The owner's device label rides the handshake for the mere-attach case: a
  // watcher that simply opens the pane hears no `pty.owner` broadcast, so this
  // key is its only source of a specific name for the take-over card. Absent
  // (never null) when there is nothing to name, and only a string is accepted.
  it("passes the handshake's owner_device through, and undefined when absent", () => {
    const devices: (string | undefined)[] = []
    const sock = new PtySocket("ws://x/pty")
    sock.onConnected = (_id, _owner, _ownerEpoch, ownerDevice) => {
      devices.push(ownerDevice)
    }
    sock.connect()
    last().open()

    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-1",
        gen: 1,
        owner: "c-9",
        owner_epoch: 7,
        owner_device: "Driver UA",
      }),
    )
    expect(devices.at(-1)).toBe("Driver UA")

    // An owner that sent no User-Agent: the key is omitted.
    last().text(
      JSON.stringify({
        event: "connected",
        id: "c-2",
        gen: 2,
        owner: "c-9",
        owner_epoch: 8,
      }),
    )
    expect(devices.at(-1)).toBeUndefined()

    // Old server: no owner key, no epoch, no device.
    last().text(JSON.stringify({ event: "connected", id: "c-3", gen: 3 }))
    expect(devices.at(-1)).toBeUndefined()
  })

  // A take-over is the ONE frame this client sends while it knows it is not the
  // owner, and the flag is what makes the server grant it. The ordinary frame
  // stays byte-identical to the one every prior version sent, so an old server
  // reading it sees exactly what it always did.
  it("flags a take-over resize, and only a take-over resize", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    const ws = last()
    ws.open()
    expect(sock.sendResize(40, 120)).toBe(true)
    expect(JSON.parse(ws.sent[0] as string)).toEqual({ rows: 40, cols: 120 })
    expect(sock.sendResize(40, 120, false)).toBe(true)
    expect(JSON.parse(ws.sent[1] as string)).toEqual({ rows: 40, cols: 120 })
    expect(sock.sendResize(40, 120, true)).toBe(true)
    expect(JSON.parse(ws.sent[2] as string)).toEqual({
      rows: 40,
      cols: 120,
      takeover: true,
    })
  })

  it("fires onOpen on each (re)open", () => {
    const sock = new PtySocket("ws://x/pty")
    let opens = 0
    sock.onOpen = () => {
      opens++
    }
    sock.connect()
    last().open()
    expect(opens).toBe(1)
  })

  it("streams server Binary frames to onBytes as Uint8Array", () => {
    const sock = new PtySocket("ws://x/pty")
    const chunks: Uint8Array[] = []
    sock.onBytes((b) => chunks.push(b))
    sock.connect()
    const ws = last()
    ws.open()
    // The server replays scrollback as the first Binary frame, then live bytes.
    ws.binary([0x68, 0x69]) // "hi" — the repaint
    ws.binary([0x21]) // "!" — a live byte
    expect(chunks.map((c) => Array.from(c))).toEqual([[0x68, 0x69], [0x21]])
  })

  it("sends stdin as a Binary (ArrayBuffer) frame when open", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    const ws = last()
    ws.open()
    sock.sendInput(new Uint8Array([1, 2, 3]))
    expect(ws.sent).toHaveLength(1)
    const frame = ws.sent[0]
    expect(frame).toBeInstanceOf(ArrayBuffer)
    expect(Array.from(new Uint8Array(frame as ArrayBuffer))).toEqual([1, 2, 3])
  })

  it("does not send stdin before the socket is open", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    // Not opened yet (readyState 0): the send guard drops it.
    sock.sendInput(new Uint8Array([9]))
    expect(last().sent).toHaveLength(0)
  })

  it("isOpen reflects the socket lifecycle (pre-open, open, closed)", () => {
    // The compose bar's Send checks this before writing so a message typed
    // while disconnected is kept (with a toast) instead of silently dropped by
    // the sendInput readyState guard.
    const sock = new PtySocket("ws://x/pty")
    expect(sock.isOpen).toBe(false)
    sock.connect()
    expect(sock.isOpen).toBe(false)
    const ws = last()
    ws.open()
    expect(sock.isOpen).toBe(true)
    sock.dispose()
    expect(sock.isOpen).toBe(false)
  })

  it("sends a resize as a Text JSON frame {rows, cols}", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    const ws = last()
    ws.open()
    expect(sock.sendResize(40, 120)).toBe(true)
    expect(ws.sent).toHaveLength(1)
    expect(JSON.parse(ws.sent[0] as string)).toEqual({ rows: 40, cols: 120 })
  })

  it("reports a resize it could NOT write, rather than dropping it silently", () => {
    // Nobody re-types a resize. The caller remembers the last size it told the
    // PTY about and skips one it believes is already there, so a frame lost to a
    // socket that is not OPEN has to come back as false or that size is booked
    // as delivered and never re-asserted.
    const sock = new PtySocket("ws://x/pty")
    expect(sock.sendResize(40, 120)).toBe(false)
    sock.connect()
    const ws = last()
    // CONNECTING, not OPEN: the state every reconnect passes through.
    expect(sock.sendResize(40, 120)).toBe(false)
    ws.open()
    expect(sock.sendResize(40, 120)).toBe(true)
    sock.dispose()
    expect(sock.sendResize(40, 120)).toBe(false)
    expect(ws.sent).toHaveLength(1)
  })

  it("reconnects after an unexpected close and receives the replay (resends nothing)", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const chunks: Uint8Array[] = []
    sock.onBytes((b) => chunks.push(b))
    let opens = 0
    sock.onOpen = () => {
      opens++
    }
    sock.connect()
    let ws = last()
    ws.open()
    expect(opens).toBe(1)
    // Drop the connection; the socket schedules a reconnect via setTimeout.
    ws.triggerClose()
    vi.advanceTimersByTime(600)
    ws = last()
    expect(ws).not.toBe(FakeWS.instances[0])
    ws.open()
    expect(opens).toBe(2)
    // The reconnect sends NOTHING on its own (no buffered subscribe) — the
    // server replays scrollback as the first Binary frame after the reopen.
    expect(ws.sent).toHaveLength(0)
    ws.binary([0x41]) // the post-reconnect repaint
    expect(chunks.map((c) => Array.from(c))).toEqual([[0x41]])
  })

  it("fires onReconnecting once when the socket drops, then onOpen on recovery", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let reconnecting = 0
    let opens = 0
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.onOpen = () => {
      opens++
    }
    sock.connect()
    last().open()
    expect(opens).toBe(1)
    expect(reconnecting).toBe(0)
    // Drop: a reconnect is scheduled, so onReconnecting fires exactly once.
    last().triggerClose()
    expect(reconnecting).toBe(1)
    // The pending reconnect timer must not re-fire onReconnecting before it opens.
    vi.advanceTimersByTime(600)
    expect(reconnecting).toBe(1)
    // Recovery re-opens the socket: onOpen signals the socket is live again.
    last().open()
    expect(opens).toBe(2)
  })

  it("does not fire onReconnecting on a user-initiated close", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let reconnecting = 0
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.connect()
    last().open()
    sock.dispose()
    vi.advanceTimersByTime(10000)
    expect(reconnecting).toBe(0)
  })

  // FLIPPED. This used to assert the socket gave up after the shared 3-attempt
  // budget and emitted `failed`. The budget is gone: it was never actually spent
  // in practice (every successful open refilled it), and giving up is the wrong
  // answer for a phone whose signal comes back in a minute. `failed` is now
  // reserved for a terminal close code, tested immediately below.
  it("never gives up on transient closes: it retries indefinitely while visible", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    // A real cycle each time: open, drop, retry. The advance stays under
    // `CONNECT_TIMEOUT_MS`, so nothing counted here is a socket abandoned for
    // never opening.
    for (let i = 0; i < 20; i++) {
      last().open()
      last().triggerClose()
      vi.advanceTimersByTime(10_000)
    }
    expect(states).not.toContain("failed")
    expect(FakeWS.instances.length).toBe(21)
    sock.dispose()
  })

  it("stops without retrying when the server closes with the provider-unavailable code", () => {
    // The provider failed to launch (missing CLI) or crashed/exited: the server
    // closes with PROVIDER_UNAVAILABLE_CLOSE, meaning "do not retry". Re-subscribing
    // would relaunch the doomed provider, so the socket must stop on the FIRST
    // such close (not loop) and surface the give-up state for the Reconnect
    // affordance — no attempt cap needed.
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    last().triggerClose(PROVIDER_UNAVAILABLE_CLOSE)
    // No reconnect scheduled, ever, and the pane's give-up state is signalled.
    vi.advanceTimersByTime(60000)
    expect(FakeWS.instances.length).toBe(before)
    expect(states.at(-1)).toBe("failed")
  })

  it("still retries after an ordinary transient close (not the provider-unavailable code)", () => {
    // A plain transport drop (code 1006) is transient — the provider may still be
    // alive server-side — so the socket reconnects to re-attach, exactly like the
    // events socket.
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    last().triggerClose(1006)
    vi.advanceTimersByTime(600)
    expect(FakeWS.instances.length).toBe(2)
  })

  it("a manual connect() after a provider-unavailable stop retries", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    const states: ConnState[] = []
    sock.onConn = (s) => states.push(s)
    sock.connect()
    last().triggerClose(PROVIDER_UNAVAILABLE_CLOSE)
    expect(states.at(-1)).toBe("failed")
    // The pane's Reconnect affordance calls connect() again; a fresh socket
    // opens, and the stop the close code set is lifted.
    const before = FakeWS.instances.length
    sock.connect()
    expect(FakeWS.instances.length).toBe(before + 1)
    expect(states.at(-1)).toBe("connecting")
  })

  it("does not reconnect and fires onGone once shouldRetry says the route is gone", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let gone = 0
    let reconnecting = 0
    sock.onGone = () => {
      gone++
    }
    sock.onReconnecting = () => {
      reconnecting++
    }
    sock.shouldRetry = () => false
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    last().triggerClose()
    // No reconnect scheduled (no new socket even after time passes) and no
    // "Reconnecting…" signal — this is a hard stop, not a retry.
    vi.advanceTimersByTime(10000)
    expect(FakeWS.instances.length).toBe(before)
    expect(reconnecting).toBe(0)
    expect(gone).toBe(1)
  })

  it("still reconnects normally when shouldRetry returns true (the default)", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    let gone = 0
    sock.onGone = () => {
      gone++
    }
    sock.connect()
    last().open()
    last().triggerClose()
    vi.advanceTimersByTime(600)
    expect(FakeWS.instances.length).toBe(2)
    expect(gone).toBe(0)
  })

  it("does not reconnect after a user-initiated close", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    const before = FakeWS.instances.length
    sock.close()
    vi.advanceTimersByTime(10000)
    expect(FakeWS.instances.length).toBe(before)
  })
})

describe("active PTY socket registry", () => {
  it("stores and clears the active socket", () => {
    expect(getActivePtySocket()).toBeNull()
    const sock = new PtySocket("ws://x/pty")
    setActivePtySocket(sock)
    expect(getActivePtySocket()).toBe(sock)
    setActivePtySocket(null)
    expect(getActivePtySocket()).toBeNull()
  })
})

describe("the take-over frame names the ghost it expects to succeed", () => {
  it("a PRESSED take-over carries no expected_owner, because a press may take from anyone", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    expect(sock.sendResize(40, 120, true)).toBe(true)
    expect(JSON.parse(last().sent.at(-1) as string)).toEqual({
      rows: 40,
      cols: 120,
      takeover: true,
    })
  })

  it("a SELF-SUCCESSION names the dead connection it believes still holds the pty", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    expect(sock.sendResize(40, 120, true, "41")).toBe(true)
    expect(JSON.parse(last().sent.at(-1) as string)).toEqual({
      rows: 40,
      cols: 120,
      takeover: true,
      expected_owner: "41",
    })
  })

  it("leaves an ordinary resize byte-identical to the one every prior version sent", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    sock.sendResize(40, 120)
    expect(last().sent.at(-1)).toBe('{"rows":40,"cols":120}')
    // And an expected owner is meaningless without the flag, so it is dropped
    // rather than sent as a claim nobody asked for.
    sock.sendResize(40, 120, false, "41")
    expect(last().sent.at(-1)).toBe('{"rows":40,"cols":120}')
  })
})

describe("the one periodic frame", () => {
  it("carries the beat number and the viewed decision together", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    expect(sock.sendBeat(7, true)).toBe(true)
    expect(JSON.parse(last().sent.at(-1) as string)).toEqual({
      beat: 7,
      viewed: true,
    })
  })

  it("is sent by a WATCHER too, with viewed false", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    sock.sendBeat(8, false)
    expect(JSON.parse(last().sent.at(-1) as string)).toEqual({
      beat: 8,
      viewed: false,
    })
  })

  it("reports a frame the socket dropped, so no deadline is started for it", () => {
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    // Never opened: the send guard discards it.
    expect(sock.sendBeat(1, false)).toBe(false)
  })

  it("surfaces the server's echo, so an answer to a stale beat is recognisable", () => {
    const sock = new PtySocket("ws://x/pty")
    const answers: number[] = []
    sock.onBeat = (n) => answers.push(n)
    sock.connect()
    const ws = last()
    ws.open()
    ws.onmessage?.({ data: JSON.stringify({ event: "beat", n: 12 }) })
    expect(answers).toEqual([12])
  })

  it("ignores a beat answer with no number rather than counting it", () => {
    const sock = new PtySocket("ws://x/pty")
    const answers: number[] = []
    sock.onBeat = (n) => answers.push(n)
    sock.connect()
    const ws = last()
    ws.open()
    ws.onmessage?.({ data: JSON.stringify({ event: "beat" }) })
    expect(answers).toEqual([])
  })
})

// THE GATE IS WIRED, AND IT PUSHES AS WELL AS BLOCKING.
//
// Nothing used to assert that `PtySocket` hands `serverValidated` to the base as
// its `canRetry`: deleting that one property left the whole suite green while
// every PTY socket in the app attached to servers it had not identified.
describe("the run-identity retry gate", () => {
  it("holds a dropped socket shut while the run has not been validated", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    clearServerValidated()
    last().triggerClose()
    // Ten minutes of retries and not one attach: the gate is what the base is
    // consulting, and it says no.
    vi.advanceTimersByTime(600000)
    expect(FakeWS.instances).toHaveLength(1)
    sock.dispose()
  })

  it("reattaches the moment the run IS validated, without waiting out the backoff", () => {
    vi.useFakeTimers()
    const sock = new PtySocket("ws://x/pty")
    sock.connect()
    last().open()
    clearServerValidated()
    last().triggerClose()
    vi.advanceTimersByTime(600000)
    expect(FakeWS.instances).toHaveLength(1)
    // No timer advance at all. The gate opening is itself the signal to try; a
    // socket left to notice on its own would sit out whatever gap its backoff
    // had grown to after everything was already healthy.
    noteServerValidated()
    expect(FakeWS.instances).toHaveLength(2)
    sock.dispose()
  })
})
