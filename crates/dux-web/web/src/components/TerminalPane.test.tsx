// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"

// TerminalPane embeds xterm.js, whose canvas rendering jsdom cannot back (see the
// note in TerminalArea.test.tsx). So we mount the REAL TerminalPane — exercising
// its actual JSX and the Reconnect button's real `onClick` — against a minimal
// xterm stub and a fake PtySocket. The fake captures the pane's own `onConn`
// handler (TerminalPane assigns `pty.onConn = …` in its wiring effect) so a test
// can drive a `failed`/`open` transition, and exposes a `connect` spy so we can
// assert the Reconnect button reconnects THIS pane's socket directly rather than
// falling back to the epoch-only store no-op that never remounts a companion
// terminal. That no-op is the regression this file guards, for BOTH kinds.

class TermStub {
  static instances: TermStub[] = []
  constructor() {
    TermStub.instances.push(this)
  }
  rows = 24
  cols = 80
  textarea = { setAttribute() {}, blur() {} }
  buffer = { active: { type: "normal" } }
  modes = {
    mouseTrackingMode: "none",
    applicationCursorKeysMode: false,
    bracketedPasteMode: false,
  }
  // The pane registers a parser-level OSC 8 gate directly on the terminal (the
  // hyperlink on/off gate), so the stub must expose registerOscHandler.
  parser = {
    registerOscHandler() {
      return { dispose() {} }
    },
  }
  loadAddon() {}
  open() {}
  onData() {
    return { dispose() {} }
  }
  attachCustomKeyEventHandler() {}
  focus() {}
  getSelection() {
    return ""
  }
  selectAll() {}
  scrollLines() {}
  scrollToBottom() {}
  clearSelection() {}
  reset() {}
  paste() {}
  write(_data: unknown, cb?: () => void) {
    cb?.()
  }
  dispose() {}
}

class FitStub {
  fit() {}
}

// The fake PtySocket. TerminalPane overwrites `onConn`/`onOpen`/`onReconnecting`/
// `onConnected` with its own handlers and calls `onBytes`, `connect`, `close`,
// `sendResize`, `sendInput`. `connect` is a spy so the Reconnect assertion can
// count calls. `emit` is a helper for tests to drive the pane's captured `onConn`.
class FakePtySocket {
  static instances: FakePtySocket[] = []
  url: string
  connect = vi.fn()
  close = vi.fn()
  sendResize = vi.fn()
  sendInput = vi.fn()
  sendViewed = vi.fn()
  // Mirrors the real socket's `isOpen` getter; a test flips it to false to
  // model a disconnected socket (the compose bar's Send checks it).
  isOpen = true
  onConnected: (id: string) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  onBytes: (cb: (b: Uint8Array) => void) => void = () => {}
  shouldRetry: () => boolean = () => true
  onGone: () => void = () => {}
  constructor(url: string) {
    this.url = url
    FakePtySocket.instances.push(this)
  }
  emit(state: ConnState): void {
    act(() => this.onConn(state))
  }
}

vi.mock("@xterm/xterm", () => ({ Terminal: TermStub }))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: FitStub }))
// Spy on sonner so the compose bar's refused-send tests can assert the user
// was told WHY the message stayed in the buffer (owner/offline/oversized).
const toastError = vi.fn()
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: (...args: unknown[]) => toastError(...args),
  }),
}))
vi.mock("@/lib/suppressViewerReports", () => ({ suppressViewerReports: () => {} }))
const notifyRegistrations: { title: () => string }[] = []
vi.mock("@/lib/agentNotifications", () => ({
  registerAgentNotifications: (_term: unknown, opts: { title: () => string }) => {
    notifyRegistrations.push(opts)
    return () => {}
  },
}))
vi.mock("@/components/MacroPopover", () => ({ MacroPopover: () => null }))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// Stub browser globals BEFORE the store module evaluates (it touches
// localStorage at import time, pulled in transitively by TerminalPane).
installStubs()
const { TerminalPane } = await import("./TerminalPane")

function makeState(offline = false, conn: ConnState = "open"): DuxState {
  return {
    conn,
    spine: {
      projects: [],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: null,
          provider: "claude",
          branch_name: "main",
          worktree_path: "/tmp/p1",
          status: "active",
          auto_reopen_enabled: false,
          terminals: [{ id: "t1", has_output: false }],
          tabs: [
            {
              id: "s1",
              provider: "claude",
              order: 0,
              working: false,
              has_output: false,
              has_live_process: true,
            },
          ],
          has_output: false,
          working: false,
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: {
      title: "dux",
      dux_version: "v1",
      show_changes_pane: false,
      always_show_tab_strip: false,
      available_providers: ["claude"],
      agent_tabs_max: 20,
    },
    offline,
    terminalEpoch: 0,
  } as unknown as DuxState
}

function installStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
}

function last(): FakePtySocket {
  const inst = FakePtySocket.instances.at(-1)
  if (!inst) throw new Error("no PtySocket constructed")
  return inst
}

beforeEach(() => {
  FakePtySocket.instances = []
  TermStub.instances = []
  notifyRegistrations.length = 0
  toastError.mockClear()
  mockState = makeState()
  installStubs()
  // The `pty.owner` epoch high-water marks are module-global; reset so a handover
  // in one test is never dropped as "stale" by a prior test's epoch.
  resetPtyOwnerEpochs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// Run the same behavioral contract for both surfaces the pane serves. The
// companion-terminal ("terminal") case is the load-bearing one: an epoch-only
// reconnect is a NO-OP for it, so this parametrization is the regression guard.
// Build the pane's (union-typed) props for either surface: an agent carries
// its owning session id, a terminal its owner ref.
function paneProps(kind: "agent" | "terminal", id: string) {
  return kind === "agent"
    ? ({ kind: "agent", id, sessionId: "s1" } as const)
    : ({ kind: "terminal", id, owner: { kind: "session", sessionId: "s1" } } as const)
}

describe.each([
  { kind: "agent" as const, id: "s1" },
  { kind: "terminal" as const, id: "t1" },
])("TerminalPane connectionLost affordance ($kind)", ({ kind, id }) => {
  it("shows the Reconnect affordance on 'failed' without doubling the spinner", () => {
    render(<TerminalPane {...paneProps(kind, id)} />)
    last().emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    expect(screen.getByText("Reconnect")).toBeTruthy()
    // The connection-lost block replaces (does not stack with) the reconnecting
    // spinner — no double overlay.
    expect(screen.queryByText("Reconnecting…")).toBeNull()
  })

  it("Reconnect calls the pane's OWN socket.connect() (not an epoch no-op)", () => {
    render(<TerminalPane {...paneProps(kind, id)} />)
    const pty = last()
    pty.emit("failed")
    // Ignore the connect() the wiring effect already fired on mount; the button
    // must fire a fresh one on THIS socket. For a companion terminal an
    // epoch-only reconnect would never reach here — that is the regression.
    pty.connect.mockClear()
    fireEvent.click(screen.getByText("Reconnect"))
    expect(pty.connect).toHaveBeenCalledTimes(1)
  })

  it("clears the affordance once the socket reopens ('open')", () => {
    render(<TerminalPane {...paneProps(kind, id)} />)
    const pty = last()
    pty.emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    pty.emit("open")
    expect(screen.queryByText("Connection lost.")).toBeNull()
  })

  it("suppresses its own connectionLost overlay while globally offline", () => {
    mockState = makeState(true)
    render(<TerminalPane {...paneProps(kind, id)} />)
    last().emit("failed")
    // The app-wide OfflineOverlay owns the offline case; the `&& !offline` gate
    // hides the per-pane affordance so the two never double up.
    expect(screen.queryByText("Connection lost.")).toBeNull()
    expect(screen.queryByText("Reconnect")).toBeNull()
  })
})

// The take-over placeholder's device naming: a `pty.owner` handover carrying the
// other device's raw User-Agent must render "Open on {parsed label}", our own claim
// echo must restore the owner view, and a non-open events socket must drop the
// specific name back to the generic copy (the stale-name-on-reconnect fix).
describe("TerminalPane take-over device naming", () => {
  const chromeMac =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"

  it("names the other device in the modal when a handover carries a device UA", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // A foreign device claims the PTY (owner id is not ours) with its UA attached.
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
  })

  it("clears the label and restores the owner view on our own claim echo", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // Learn our own connection id so an echo with that id reads as ours.
    act(() => last().onConnected("conn-self"))
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    // Our own claim echoes back -> we are the owner again; the placeholder (and its
    // device name) disappears entirely.
    act(() => notifyPtyOwner("s1", "conn-self", undefined, chromeMac))
    expect(screen.queryByText(/Open on/)).toBeNull()
    expect(screen.queryByText("Active on another device")).toBeNull()
  })

  it("drops the specific name to the generic copy when events socket is not open", () => {
    const { rerender } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    // The events socket drops (conn !== "open"): the specific-but-now-possibly-stale
    // device name is cleared, falling back to the always-correct generic copy.
    mockState = makeState(false, "closed")
    act(() => rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />))
    expect(screen.queryByText("Open on Chrome on macOS")).toBeNull()
    expect(screen.getByText("Active on another device")).toBeTruthy()
  })
})

// T9: a project terminal must resolve its OWNER (a project), not scan sessions.
// With the old session-only resolution, its desktop notifications were titled
// "Agent", its readiness never latched (hasOutput stayed false), and the pane
// opened the session-nested PTY route (a silent 404).
describe("TerminalPane project-terminal owner resolution", () => {
  function projectState(hasOutput: boolean): DuxState {
    const base = makeState()
    return {
      ...base,
      spine: {
        projects: [
          {
            id: "p1",
            name: "Repo",
            terminals: [
              {
                id: "pt-1",
                label: "Terminal 2",
                has_output: hasOutput,
                foreground_cmd: null,
              },
            ],
          },
        ],
        sessions: [],
        sidebar: { groups: [], agentless_start: null },
      },
    } as unknown as DuxState
  }

  it("opens the PROJECT-nested PTY socket URL", () => {
    mockState = projectState(true)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(last().url).toMatch(/\/ws\/projects\/p1\/terminals\/pt-1\/pty$/)
  })

  it("titles desktop notifications with the project name, not 'Agent'", () => {
    mockState = projectState(true)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(notifyRegistrations).toHaveLength(1)
    expect(notifyRegistrations[0].title()).toBe("Repo")
  })

  it("reads readiness from the project's own TerminalView", () => {
    // has_output true on the PROJECT's terminal: the readiness spinner is
    // skipped. The old session scan saw `undefined ?? false` forever, so the
    // "Launching terminal…" card never cleared.
    mockState = projectState(true)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(screen.queryByText("Launching terminal…")).toBeNull()
  })

  it("shows the launch spinner while the project terminal has no output yet", () => {
    mockState = projectState(false)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(screen.getByText("Launching terminal…")).toBeTruthy()
  })
})

// The mobile compose bar (the `ui.compose_bar` preference, default on): the
// third row of the mobile shell, whose Send delivers the buffered message plus
// a submitting Enter to the PTY through the pure `composeSendBytes` rules.
// `useIsMobile` reads `window.innerWidth`, so shrinking it below the 768px
// breakpoint is how these tests mount the mobile shell.
describe("TerminalPane mobile compose bar", () => {
  const desktopWidth = window.innerWidth
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  const composeTextarea = () =>
    screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement
  const sendButton = () => screen.getByRole("button", { name: "Send" })
  const bytesOf = (call: unknown[]) =>
    new TextDecoder().decode(call[0] as Uint8Array)

  it("renders on mobile by default (preference absent falls back to on)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea()).toBeTruthy()
    expect(sendButton()).toBeTruthy()
  })

  it("does not render when the ui.compose_bar preference is off", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: boolean }).compose_bar =
      false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
  })

  it("does not render on desktop", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
  })

  it("Send writes the buffer plus a submitting CR to the PTY", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    fireEvent.change(composeTextarea(), { target: { value: "ls -la" } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("ls -la\r")
  })

  it("an empty Send writes a bare Enter (CR)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("\r")
  })

  it("a multiline Send is one macro-style write: Alt+Enter newlines, then CR", () => {
    // The macro convention (macroPayloadBytes): newlines are the Alt+Enter
    // keystroke (ESC CR, newline-without-submit), the trailing bare CR is the
    // submitting Enter. One write; line break and Enter are distinct keys.
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    fireEvent.change(composeTextarea(), { target: { value: "a\nb" } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("a\x1b\rb\r")
  })

  it("ignores bracketed paste: the payload is keystrokes even when the app negotiated it", () => {
    // Deliberate: wrapping the body as a paste made Ink-based TUIs (Claude
    // Code) swallow a same-chunk CR inside their paste handling, so Send typed
    // the message but never submitted. The keystroke stream has no paste for a
    // guard to interfere with, so bracketedPasteMode is simply not consulted.
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    term.modes.bracketedPasteMode = true
    fireEvent.change(composeTextarea(), { target: { value: "a\nb" } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("a\x1b\rb\r")
  })

  it("an empty Send is a single bare CR regardless of bracketed paste", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    term.modes.bracketedPasteMode = true
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("\r")
  })

  it("a non-owner's Send writes nothing, keeps the buffer, and toasts why", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // A foreign device takes over: this view is demoted to a read-only viewer.
    act(() => notifyPtyOwner("s1", "conn-other", undefined, undefined))
    fireEvent.change(composeTextarea(), { target: { value: "stolen" } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).not.toHaveBeenCalled()
    // The draft is KEPT so a take-over can retry it, and the refusal is
    // explained instead of silently swallowed.
    expect(composeTextarea().value).toBe("stolen")
    expect(toastError).toHaveBeenCalledWith(
      "Another device is driving this terminal. Take over to send.",
      expect.anything(),
    )
  })

  it("a Send while the socket is down keeps the buffer and toasts", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // Model a dropped connection: the real socket's isOpen getter reads the
    // WebSocket readyState; the fake exposes it as a writable flag.
    pty.isOpen = false
    fireEvent.change(composeTextarea(), { target: { value: "while offline" } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).not.toHaveBeenCalled()
    expect(composeTextarea().value).toBe("while offline")
    expect(toastError).toHaveBeenCalledWith(
      "Not connected right now. Your message was kept.",
      expect.anything(),
    )
  })

  it("an oversized Send keeps the buffer and toasts instead of writing", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // One byte over the 2 MiB client-side cap; the server would abort the
    // whole PTY socket on a genuinely oversized frame, so the client refuses.
    const huge = "a".repeat(2 * 1024 * 1024 + 1)
    fireEvent.change(composeTextarea(), { target: { value: huge } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).not.toHaveBeenCalled()
    expect(composeTextarea().value).toBe(huge)
    expect(toastError).toHaveBeenCalled()
  })

  it("keeps in-progress text across a compose-bar unmount (pref flip off and on)", () => {
    goMobile()
    const { rerender } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    fireEvent.change(composeTextarea(), { target: { value: "draft survives" } })
    // The preference flips off: the bar unmounts. The buffer lives in
    // TerminalPane state, not the bar, so it must survive the round trip.
    const off = makeState()
    ;(off.bootstrap as unknown as { compose_bar?: boolean }).compose_bar = false
    mockState = off
    rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    mockState = makeState()
    rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea().value).toBe("draft survives")
  })
})

// The tap-to-focus redirect, driven through real touch events on the terminal
// container (jsdom accepts plain {clientX, clientY} objects in the touch
// lists). touchend must be read via `changedTouches` (the `touches` list is
// empty once the finger lifts).
describe("TerminalPane tap-to-focus redirect", () => {
  const desktopWidth = window.innerWidth
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  const container = () => screen.getByTestId("terminal-container")
  const composeTextarea = () =>
    screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement
  const tap = (el: HTMLElement) => {
    fireEvent.touchStart(el, {
      touches: [{ clientX: 10, clientY: 10 }],
    })
    // fireEvent returns false iff preventDefault was called on the touchend,
    // which is the mechanism that stops xterm's synthetic-mousedown focus.
    return !fireEvent.touchEnd(el, {
      touches: [],
      changedTouches: [{ clientX: 10, clientY: 10 }],
    })
  }

  it("a tap preventDefaults and focuses the compose textarea (owner, pref on)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const prevented = tap(container())
    expect(prevented).toBe(true)
    expect(document.activeElement).toBe(composeTextarea())
  })

  it("does not intercept the tap when the preference is off", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: boolean }).compose_bar =
      false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // No preventDefault: the synthetic mouse events flow and xterm focuses its
    // hidden textarea exactly as before the compose bar existed.
    expect(tap(container())).toBe(false)
  })

  it("forwards a synthetic SGR click to a mouse-tracking app AND focuses compose", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = FakePtySocket.instances.at(-1)
    const term = TermStub.instances.at(-1)
    if (!pty || !term) throw new Error("no pty/term constructed")
    // The app in the PTY has grabbed the mouse: the swallowed tap must still
    // reach it as a left click (press + release) at the tapped cell, or
    // tap-driven TUIs go dead with the compose bar up.
    term.modes.mouseTrackingMode = "sgr"
    const prevented = tap(container())
    expect(prevented).toBe(true)
    expect(document.activeElement).toBe(composeTextarea())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    const bytes = new TextDecoder().decode(
      pty.sendInput.mock.calls[0][0] as Uint8Array,
    )
    // jsdom rects/sizes are all 0, so the drag-path math degrades to the
    // 1px-per-cell guard: cell = floor(10 / 1) + 1 = 11 on both axes.
    expect(bytes).toBe("\x1b[<0;11;11M\x1b[<0;11;11m")
  })
})
