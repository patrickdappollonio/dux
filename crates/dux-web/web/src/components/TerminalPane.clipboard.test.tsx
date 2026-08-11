// @vitest-environment jsdom
//
// Pasting an image out of the clipboard, driven through the real pane. Same
// harness as `TerminalPane.filedrop.test.tsx` and for the same reason: the only
// things stubbed are the ones that genuinely cannot run here (xterm needs a
// canvas, and there is no network), so the gating, the ordering, the per-CLI
// form and the reporting are all the real code.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  createEvent,
  fireEvent,
  render,
  screen,
} from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"
import { stubCoarsePointer, type MatchMediaStub } from "@/test/matchMedia"

/// The same xterm stand-in the drop suite uses: `paste()` does what xterm's
/// really does, so an assertion can be made on the bytes that reach the socket
/// rather than on a call that was merely made.
class TermStub {
  static instances: TermStub[] = []
  static pastes: string[] = []
  private dataHandler: ((s: string) => void) | null = null
  options: Record<string, unknown>
  constructor(options?: Record<string, unknown>) {
    this.options = options ?? {}
    TermStub.instances.push(this)
  }
  rows = 24
  cols = 80
  /// A REAL textarea, appended to the container by `open()`, standing in for
  /// xterm's own hidden input. It is what makes the capture phase real: xterm
  /// registers its paste handler on this element, INSIDE the container dux
  /// listens on, so a paste event has to target a descendant or an
  /// ancestor-capture listener and a plain bubble listener are indistinguishable
  /// and the tests prove nothing about the ordering that the feature rests on.
  textarea: HTMLTextAreaElement = document.createElement("textarea")
  /// A stand-in for xterm's own `paste` handler, registered on the textarea in
  /// `open()` exactly as xterm registers its real one. What it records is the
  /// single most valuable fact in this suite: whether an ordinary text paste
  /// still REACHES xterm. Cancelling the event, or merely stopping its
  /// propagation, would kill every text paste in a real browser while leaving
  /// `defaultPrevented` false, and nothing else here can see that.
  static xtermPastes: ClipboardEvent[] = []
  buffer = { active: { type: "normal" } }
  modes = {
    mouseTrackingMode: "none",
    applicationCursorKeysMode: false,
    bracketedPasteMode: false,
  }
  parser = {
    registerOscHandler() {
      return { dispose() {} }
    },
  }
  loadAddon() {}
  open(parent: HTMLElement) {
    parent.appendChild(this.textarea)
    this.textarea.addEventListener("paste", (e) => {
      TermStub.xtermPastes.push(e as ClipboardEvent)
    })
  }
  onResize() {
    return { dispose() {} }
  }
  onData(cb: (s: string) => void) {
    this.dataHandler = cb
    return { dispose() {} }
  }
  // xterm's other output channel: an X10-encoded mouse report goes out here,
  // not through onData. Subscribable so the pane's mount effect completes.
  onBinary() {
    return { dispose() {} }
  }
  /// Kept rather than dropped, so a test can drive the real chord handler: the
  /// text-paste hatch is a KEY event arming a latch a PASTE event consumes, and
  /// only exercising both halves proves the two meet.
  keyHandler: ((e: KeyboardEvent) => boolean) | null = null
  attachCustomKeyEventHandler(cb: (e: KeyboardEvent) => boolean) {
    this.keyHandler = cb
  }
  focus() {}
  getSelection() {
    return ""
  }
  selectAll() {}
  scrollLines() {}
  scrollToBottom() {}
  clearSelection() {}
  reset() {}
  paste(text: string) {
    TermStub.pastes.push(text)
    let out = text.replace(/\r?\n/g, "\r")
    if (this.modes.bracketedPasteMode) out = `\x1b[200~${out}\x1b[201~`
    this.dataHandler?.(out)
  }
  write(_data: unknown, cb?: () => void) {
    cb?.()
  }
  dispose() {}
}

class FitStub {
  fit() {}
  proposeDimensions() {
    return { rows: 24, cols: 80 }
  }
}

class FakePtySocket {
  static instances: FakePtySocket[] = []
  url: string
  connect = vi.fn()
  close = vi.fn()
  sendResize = vi.fn(() => true)
  sendInput = vi.fn()
  sendViewed = vi.fn()
  isOpen = true
  onConnected: (id: string) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: () => void = () => {}
  onBytes: (cb: (b: Uint8Array) => void) => void = () => {}
  shouldRetry: () => boolean = () => true
  onGone: () => void = () => {}
  constructor(url: string) {
    this.url = url
    FakePtySocket.instances.push(this)
  }
}

vi.mock("@xterm/xterm", () => ({ Terminal: TermStub }))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: FitStub }))
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}))
vi.mock("@/lib/suppressViewerReports", () => ({
  suppressViewerReports: () => {},
}))
vi.mock("@/components/MacroPopover", () => ({ MacroPopover: () => null }))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

const uploadDroppedFile = vi.fn()
vi.mock("@/lib/fileDropApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/fileDropApi")>()
  return {
    ...actual,
    uploadDroppedFile: (...a: unknown[]) => uploadDroppedFile(...a),
  }
})

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

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
    vi.fn(() => Promise.reject(new Error("no network in tests"))),
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

installStubs()
const { TerminalPane } = await import("./TerminalPane")
const { toast } = await import("sonner")

function makeState(): DuxState {
  return {
    conn: "open",
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
      terminals: [],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: {
      title: "dux",
      dux_version: "v1",
      show_changes_pane: false,
      always_show_tab_strip: false,
      available_providers: ["claude"],
      agent_tabs_max: 20,
      status_clear_seconds: 6,
      compose_bar: "auto",
      mobile_top_bar: true,
      mobile_accessory_bar: true,
      file_drop_max_bytes: 1024,
      upload_pasted_text_chars: 1000,
      provider_drop_paste: {
        claude: { form: "bare", command_name: "claude" },
        codex: { form: "single_quoted", command_name: "codex" },
      },
    },
    offline: false,
    terminalEpoch: 0,
  } as unknown as DuxState
}

/// A workspace whose focused tab runs `provider`, with the agent's own provider
/// deliberately set to something else so reading the wrong one cannot produce
/// the right bytes by luck.
function stateRunning(provider: string): DuxState {
  const s = makeState()
  const session = s.spine!.sessions[0]
  session.provider = provider === "codex" ? "claude" : "codex"
  session.tabs[0].provider = provider
  return s
}

function saved(name: string, savedName = name) {
  return {
    path: `/tmp/p1/.dux/uploads/${savedName}`,
    saved_name: savedName,
    requested_name: name,
    folder: "/tmp/p1/.dux/uploads",
    folder_label: "~/p1/.dux/uploads",
    renamed: name !== savedName,
  }
}

function png(name: string) {
  return new File([new Uint8Array([1, 2, 3])], name, { type: "image/png" })
}

/// A `DataTransferItem` stand-in, as the browser hands one over on a paste.
function imageItem(file: File) {
  return { kind: "file", type: file.type, getAsFile: () => file }
}

function textItem(text = "hello") {
  return {
    kind: "string",
    type: "text/plain",
    getAsFile: () => null,
    getAsString: (cb: (s: string) => void) => cb(text),
  }
}

/// Dispatch a real `paste` event at `el` and return it, so the test can assert
/// on `defaultPrevented`: whether dux took the paste over is exactly the
/// question "did the browser's own paste still happen".
async function paste(el: Element, items: unknown[], text = "") {
  const event = createEvent.paste(el, {
    clipboardData: {
      items,
      types: items.map((i) => (i as { type: string }).type),
      // The synchronous flavour read, which is the only way the long-text
      // decision can see the contents in time to cancel the event: a
      // `DataTransferItem` of kind `string` yields its text through an async
      // callback, and by then the paste has already happened.
      getData: (type: string) => (type === "text/plain" ? text : ""),
    },
  })
  await act(async () => {
    fireEvent(el, event)
    // Let the sequential upload loop run to completion.
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()
  })
  return event
}

/// Where a paste over the terminal actually LANDS: xterm's hidden textarea,
/// inside the container dux's capture listener is on. Dispatching at the
/// container itself would make the capture registration indistinguishable from
/// a bubble one, which is the ordering the whole design rests on.
function terminalHost(): Element {
  return TermStub.instances.at(-1)!.textarea
}

/// The chord handler xterm hands its keys to, as the pane installed it.
function pressKey(init: Partial<KeyboardEvent> & { code: string }): boolean {
  const handler = TermStub.instances.at(-1)!.keyHandler!
  let prevented = false
  const event = {
    type: "keydown",
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    keyCode: 0,
    preventDefault: () => {
      prevented = true
    },
    stopPropagation: () => {},
    ...init,
  } as unknown as KeyboardEvent
  handler(event)
  return prevented
}

/// Everything the pane actually WROTE TO THE SOCKET, decoded, in order.
function sentToSocket(): string[] {
  const socket = FakePtySocket.instances.at(-1)
  if (!socket) return []
  const decoder = new TextDecoder()
  return socket.sendInput.mock.calls.map(([bytes]) =>
    decoder.decode(bytes as Uint8Array),
  )
}

beforeEach(() => {
  FakePtySocket.instances = []
  TermStub.instances = []
  TermStub.pastes = []
  TermStub.xtermPastes = []
  uploadDroppedFile.mockReset()
  vi.mocked(toast.success).mockClear()
  vi.mocked(toast.warning).mockClear()
  vi.mocked(toast.error).mockClear()
  vi.mocked(toast.loading).mockClear()
  mockState = makeState()
  installStubs()
  resetPtyOwnerEpochs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("pasting an image onto an agent", () => {
  it("saves it and sends its path, in the form the running CLI needs", async () => {
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    mockState = stateRunning("claude")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const event = await paste(terminalHost(), [imageItem(png("image.png"))])

    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
    expect((uploadDroppedFile.mock.calls[0][0] as File).name).toBe("image.png")
    // Claude's configured form is bare, so the path goes out untouched. The
    // assertion is at the SOCKET, after xterm's own preparation, because a
    // `term.paste()` call proves nothing about what the agent receives.
    expect(sentToSocket()).toEqual(["/tmp/p1/.dux/uploads/image.png "])
    // The image bytes never reached xterm: the only thing pasted is the path.
    expect(TermStub.pastes).toEqual(["/tmp/p1/.dux/uploads/image.png "])
    // And the browser's own paste was cancelled, so nothing else landed.
    expect(event.defaultPrevented).toBe(true)
    // Cancelled AND stopped: the capture listener runs on the container before
    // the event ever reaches xterm's handler on the textarea inside it, so
    // xterm never sees the image paste at all.
    expect(TermStub.xtermPastes).toEqual([])
    expect(vi.mocked(toast.success)).toHaveBeenCalled()
  })

  it("single-quotes the path for a pane running codex", async () => {
    // The same per-provider form resolution the drop path uses, reached
    // through the paste gesture. A path with a space is silently ignored by
    // codex unless it lexes as exactly one token.
    uploadDroppedFile.mockResolvedValue({
      path: "/tmp/p1/.dux/up loads/image.png",
      saved_name: "image.png",
      requested_name: "image.png",
      folder: "/tmp/p1/.dux/up loads",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateRunning("codex")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await paste(terminalHost(), [imageItem(png("image.png"))])

    expect(sentToSocket()).toEqual(["'/tmp/p1/.dux/up loads/image.png' "])
  })

  it("keeps a non-Latin file name all the way to the server", async () => {
    const name = "スクリーンショット.png"
    uploadDroppedFile.mockResolvedValue(saved(name))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await paste(terminalHost(), [imageItem(png(name))])

    expect((uploadDroppedFile.mock.calls[0][0] as File).name).toBe(name)
    expect(sentToSocket()).toEqual([`/tmp/p1/.dux/uploads/${name} `])
  })

  it("takes the image when the clipboard carries text alongside it", async () => {
    // A screenshot copied out of an application arrives as an image PLUS a
    // text snapshot. Letting both through would paste the path and then dump
    // the text after it.
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const event = await paste(terminalHost(), [
      textItem("<b>some markup</b>"),
      imageItem(png("image.png")),
    ])

    expect(sentToSocket()).toEqual(["/tmp/p1/.dux/uploads/image.png "])
    expect(event.defaultPrevented).toBe(true)
  })
})

describe("an ordinary text paste is untouched", () => {
  it("is left entirely to xterm: nothing uploaded, nothing cancelled", async () => {
    // The pane deliberately lets Ctrl+v fall through so the browser's native
    // paste event feeds xterm's own handler (secure-context-free, which is why
    // it is the robust path over plain HTTP). Image handling must not disturb
    // that, so this asserts the event survives untouched.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const event = await paste(terminalHost(), [textItem("ls -la")])

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.pastes).toEqual([])
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled()
    expect(vi.mocked(toast.warning)).not.toHaveBeenCalled()
    // The one that matters: the event REACHED xterm. `defaultPrevented` being
    // false is not enough on its own, because a stray `stopPropagation` on the
    // container would kill every text paste in a real browser and leave that
    // flag false the whole time.
    expect(TermStub.xtermPastes).toHaveLength(1)
    expect(TermStub.xtermPastes[0]).toBe(event)
  })

  it("is left alone for an empty clipboard too", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const event = await paste(terminalHost(), [])
    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.xtermPastes).toEqual([event])
  })
})

describe("Ctrl+Shift+v forces a text paste", () => {
  // The escape hatch out of image-wins. Copying rich content (a spreadsheet
  // range, a slide) routinely puts an `image/png` on the clipboard beside the
  // text, and without this the text would be unreachable.
  it("skips image handling and hands the whole event to xterm", async () => {
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)

    // The chord is not swallowed: it returns to the browser so the native
    // paste event fires, which is the only place clipboard contents exist.
    expect(pressKey({ code: "KeyV", ctrlKey: true, shiftKey: true })).toBe(false)
    const event = await paste(terminalHost(), [
      textItem("42\t43"),
      imageItem(png("image.png")),
    ])

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.xtermPastes).toEqual([event])
  })

  it("works on a Mac, where the chord carries Cmd and not Ctrl", async () => {
    // `classifyClipboardKey` sends every Cmd combo straight to `passthrough`
    // before any other rule, so the hatch cannot live inside it.
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)

    pressKey({ code: "KeyV", metaKey: true, shiftKey: true })
    await paste(terminalHost(), [imageItem(png("image.png"))])

    expect(uploadDroppedFile).not.toHaveBeenCalled()
  })

  it("is one keystroke only: a plain Ctrl+v after it still takes the image", async () => {
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)

    pressKey({ code: "KeyV", ctrlKey: true, shiftKey: true })
    await paste(terminalHost(), [imageItem(png("image.png"))])
    expect(uploadDroppedFile).not.toHaveBeenCalled()

    pressKey({ code: "KeyV", ctrlKey: true })
    await paste(terminalHost(), [imageItem(png("image.png"))])
    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
  })

  it("does not leave the hatch armed when the chord produces no paste at all", async () => {
    // A chord the OS refuses to complete (no clipboard read, no paste event)
    // must not disarm image handling for whatever is pasted next.
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    pressKey({ code: "KeyV", ctrlKey: true, shiftKey: true })
    // One turn of the task queue, which is where the latch's expiry sits. The
    // browser dispatches a real paste as the keydown's default action, BEFORE
    // yielding here, so nothing legitimate is ever expired out from under.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    await paste(terminalHost(), [imageItem(png("image.png"))])
    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
  })
})

describe("the paste report honors the configured dismiss window", () => {
  it("reads the setting that arrived after mount, not the one missing at mount", async () => {
    // The paste listener is registered in the mount effect, so it closes over
    // the MOUNT render. Rendering is not gated on the bootstrap document, so at
    // mount the setting is usually absent, and reading it straight out of the
    // closure pinned every clipboard toast to the default for the life of the
    // pane while the DROP path (a JSX handler, rebuilt every render) followed
    // the setting. The two must not disagree.
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    mockState = { ...makeState(), bootstrap: undefined } as unknown as DuxState
    const view = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)

    mockState = makeState()
    mockState.bootstrap!.status_clear_seconds = 30
    await act(async () => {
      view.rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    })
    await paste(terminalHost(), [imageItem(png("image.png"))])

    const options = vi.mocked(toast.success).mock.calls.at(-1)?.[1] as {
      duration: number
    }
    // Success is the 1x rung of the graded window, so this is the configured
    // 30s and not the 6s default.
    expect(options.duration).toBe(30_000)
  })
})

describe("when an image paste cannot be taken", () => {
  it("refuses it for a client that does not hold input, and saves nothing", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))
    const event = await paste(terminalHost(), [imageItem(png("image.png"))])

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(sentToSocket()).toEqual([])
    // Cancelled, so the image does not fall through to xterm either.
    expect(event.defaultPrevented).toBe(true)
    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("Take over")
    expect(TermStub.xtermPastes).toEqual([])
  })

  it("lets a viewer's paste chord reach the browser, so the refusal is reachable at all", async () => {
    // The refusal above is only ever seen if a native paste event fires, and
    // the key handler used to swallow the chord for a non-owner: no event, no
    // capture listener, no toast. Only on Linux and Windows, since `Cmd+v`
    // classifies as passthrough and never reached that branch, so the bug was
    // platform-asymmetric on top of being silent.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))

    expect(pressKey({ code: "KeyV", ctrlKey: true })).toBe(false)
    expect(pressKey({ code: "KeyV", ctrlKey: true, shiftKey: true })).toBe(false)
  })

  it("still keeps a viewer's TEXT paste off the wire", async () => {
    // The safety property the fall-through rests on. xterm's own paste handler
    // ends in `triggerDataEvent`, which is the pane's `onData` subscription,
    // and that returns early for a non-owner. `TermStub.paste` reproduces that
    // chain, so this is the real gate and not a restatement of it.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))

    const event = await paste(terminalHost(), [textItem("rm -rf /")])
    expect(event.defaultPrevented).toBe(false)
    // The event reaches xterm exactly as it would for an owner...
    expect(TermStub.xtermPastes).toEqual([event])
    // ...and xterm handing that text on still writes nothing to the socket.
    act(() => TermStub.instances.at(-1)!.paste("rm -rf /"))
    expect(sentToSocket()).toEqual([])
  })

  it("reports the server's own refusal when the image is too big", async () => {
    const { FileDropApiError } = await import("@/lib/fileDropApi")
    uploadDroppedFile.mockRejectedValue(
      new FileDropApiError(
        "That file is over the 1024 byte limit for a dropped file.",
        413,
      ),
    )
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await paste(terminalHost(), [imageItem(png("huge.png"))])

    expect(sentToSocket()).toEqual([])
    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("huge.png")
    expect(message).toContain("over the 1024 byte limit")
  })

  it("does nothing at all while the upload feature is switched off", async () => {
    mockState = makeState()
    mockState.bootstrap!.file_drop_max_bytes = 0
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const event = await paste(terminalHost(), [imageItem(png("image.png"))])

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled()
  })
})

describe("pasting an image while the mobile compose bar is the typing surface", () => {
  // Two signals, because the compose bar needs both: `useIsMobile` reads
  // `window.innerWidth` for the mobile SHELL, and the bar inside it is gated on
  // `pointer: coarse` (see `hooks/use-coarse-pointer.ts`). A real phone reports
  // both, so these set both for the duration.
  const desktopWidth = window.innerWidth
  let pointerStub: MatchMediaStub | null = null
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: 420,
    })
    pointerStub = stubCoarsePointer()
  })
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: desktopWidth,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  it("puts the path in the DRAFT and sends nothing", async () => {
    uploadDroppedFile.mockResolvedValue(saved("image.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: "look at " } })

    const event = await paste(box, [imageItem(png("image.png"))])

    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
    // The draft grew; nothing went to the PTY. On a phone the compose bar is
    // the typing surface, and a paste that submitted would fire a half-written
    // message.
    expect(box.value).toBe("look at /tmp/p1/.dux/uploads/image.png ")
    expect(sentToSocket()).toEqual([])
    expect(TermStub.pastes).toEqual([])
    expect(event.defaultPrevented).toBe(true)
    // And the toast says where the path actually went, rather than claiming
    // the agent already has it.
    const message = vi.mocked(toast.success).mock.calls[0][0] as string
    expect(message).toContain("message")
    expect(message).not.toContain("sent")
  })

  it("reports the file as stranded when the box goes away mid-upload", async () => {
    // The sink outlives the bar (the upload is in flight while a rotation or a
    // preference flip unmounts it), and the insert would still land in the
    // surviving draft state. What would not survive is the report: "added its
    // path to your message" with no message box on screen is a lie about where
    // the file went, so the sink refuses and names the path instead.
    let release: (v: unknown) => void = () => {}
    uploadDroppedFile.mockReturnValue(
      new Promise((resolve) => {
        release = resolve
      }),
    )
    const view = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: "look at " } })

    const event = createEvent.paste(box, {
      clipboardData: {
        items: [imageItem(png("image.png"))],
        types: ["image/png"],
        getData: () => "",
      },
    })
    await act(async () => void fireEvent(box, event))

    // The bar goes away while the upload is still in flight.
    mockState = { ...mockState, bootstrap: { ...mockState.bootstrap!, compose_bar: "never" } }
    await act(async () => {
      view.rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    })
    await act(async () => {
      release(saved("image.png"))
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
    })

    // Nothing was written anywhere, and the report says so with the full path.
    expect(sentToSocket()).toEqual([])
    expect(TermStub.pastes).toEqual([])
    const message = vi.mocked(toast.warning).mock.calls.at(-1)?.[0] as string
    expect(message).toContain("/tmp/p1/.dux/uploads/image.png")
    expect(message).toContain("message box closed")
  })

  it("leaves a text paste in the compose box alone", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement
    const event = await paste(box, [textItem("some text")])
    expect(uploadDroppedFile).not.toHaveBeenCalled()
    // Not cancelled, so the browser inserts the text into the textarea itself,
    // exactly as it does for any other paste into any other input.
    expect(event.defaultPrevented).toBe(false)
  })
})

describe("pasting a very long text onto an agent", () => {
  // The same journey as an image, entered by a different trigger, and for the
  // user's own reason: an agent's context window is finite, but it can read a
  // document when it needs to. A path costs almost nothing; a wall of text
  // costs the context whether the agent needed all of it or not.

  /// `n` characters of ordinary prose-shaped text.
  function long(n: number, ch = "x") {
    return ch.repeat(n)
  }

  it("saves it as a .txt file and sends the path, never the text", async () => {
    uploadDroppedFile.mockResolvedValue(saved("pasted-x.txt"))
    mockState = stateRunning("claude")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const text = long(5000)
    const event = await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
    const uploaded = uploadDroppedFile.mock.calls[0][0] as File
    expect(uploaded.name).toMatch(/^pasted-\d{4}-\d{2}-\d{2}-\d{6}\.txt$/)
    expect(await uploaded.text()).toBe(text)
    // The PATH went out, and the 5000 characters did not.
    expect(sentToSocket()).toEqual(["/tmp/p1/.dux/uploads/pasted-x.txt "])
    expect(TermStub.pastes).toEqual(["/tmp/p1/.dux/uploads/pasted-x.txt "])
    // Cancelled AND stopped, so xterm never sees the text at all.
    expect(event.defaultPrevented).toBe(true)
    expect(TermStub.xtermPastes).toEqual([])
    // And the report says what happened, in the user's terms.
    const message = vi.mocked(toast.success).mock.calls.at(-1)![0] as string
    expect(message).toContain("5000 characters")
    expect(message).toContain("saved it as a file")
    expect(message).toContain("pasted-x.txt")
  })

  it("leaves a paste under the threshold entirely to xterm", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const text = long(999)
    const event = await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.xtermPastes).toEqual([event])
  })

  it("pastes long text verbatim into a TERMINAL, at any length", async () => {
    // A long paste into a shell is a command or a heredoc. Turning it into a
    // file would destroy what the user meant, so a terminal never does.
    render(
      <TerminalPane
        kind="terminal"
        id="t1"
        owner={{ kind: "session", session_id: "s1" }}
      />,
    )
    const text = long(50_000)
    const event = await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.xtermPastes).toEqual([event])
  })

  it("is switched off by a threshold of 0", async () => {
    mockState = makeState()
    mockState.bootstrap!.upload_pasted_text_chars = 0
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const text = long(50_000)
    const event = await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
  })

  it("is switched off for a server that never published the setting", async () => {
    // Not yet known is not enabled, the same rule `file_drop_max_bytes`
    // follows: nothing surprising happens to a paste until dux has said the
    // feature is there.
    mockState = makeState()
    delete mockState.bootstrap!.upload_pasted_text_chars
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const text = long(50_000)
    await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).not.toHaveBeenCalled()
  })

  it("hands long text straight to xterm on Ctrl+Shift+v", async () => {
    // One hatch for both triggers. The chord already means "give it to me
    // literally" for image-wins, and it means the same thing here.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(pressKey({ code: "KeyV", ctrlKey: true, shiftKey: true })).toBe(false)
    const text = long(50_000)
    const event = await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.xtermPastes).toEqual([event])
  })

  it("refuses it for a client that does not hold input, and saves nothing", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))
    const text = long(50_000)
    const event = await paste(terminalHost(), [textItem(text)], text)

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(sentToSocket()).toEqual([])
    expect(event.defaultPrevented).toBe(true)
    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("Take over")
    // Its OWN toast id. On the image refusal's id the two would replace each
    // other, so a viewer who pastes a screenshot and then a wall of text sees
    // one message and is told nothing about the other.
    const options = vi.mocked(toast.error).mock.calls[0][1] as { id: string }
    expect(options.id).toBe("clipboard-text-paste")
  })

  it("keeps the image refusal on its own toast id", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))
    await paste(terminalHost(), [imageItem(png("shot.png"))])

    const options = vi.mocked(toast.error).mock.calls[0][1] as { id: string }
    expect(options.id).toBe("clipboard-image-paste")
  })

  it("fires at exactly one character over a threshold the server chose", async () => {
    // Not the shipped default, so this pins that the number really is read off
    // the bootstrap document rather than baked in, and pins the boundary at the
    // same time: `limit` is typed, `limit + 1` becomes a file.
    uploadDroppedFile.mockResolvedValue(saved("pasted-x.txt"))
    mockState = makeState()
    mockState.bootstrap!.upload_pasted_text_chars = 2500
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)

    const atLimit = long(2500)
    const first = await paste(terminalHost(), [textItem(atLimit)], atLimit)
    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(first.defaultPrevented).toBe(false)

    const overLimit = long(2501)
    const second = await paste(terminalHost(), [textItem(overLimit)], overLimit)
    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
    expect(second.defaultPrevented).toBe(true)
    const message = vi.mocked(toast.success).mock.calls.at(-1)![0] as string
    expect(message).toContain("2501 characters")
  })

  it("leaves a paste carrying only text/html alone", async () => {
    // Copying out of a rich editor puts `text/html` on the clipboard, and a
    // browser that offers no `text/plain` flavour beside it gives `getData` an
    // empty string. There is no text for dux to file away, so the paste is
    // xterm's, however large the markup is.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const htmlItem = {
      kind: "string",
      type: "text/html",
      getAsFile: () => null,
      getAsString: (cb: (s: string) => void) => cb(long(50_000)),
    }
    const event = await paste(terminalHost(), [htmlItem], "")

    expect(uploadDroppedFile).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
    expect(TermStub.xtermPastes).toEqual([event])
  })

  it("files a long paste away from the mobile compose box too, into the DRAFT", async () => {
    // The compose bar is IN scope, deliberately. A paste that large is a
    // document whichever surface receives it, and the path joining the draft is
    // better than either dumping the text into the message or refusing it: the
    // user can still write around the path before pressing Send.
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 420 })
    const pointerStub = stubCoarsePointer()
    try {
      uploadDroppedFile.mockResolvedValue(saved("pasted-x.txt"))
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const box = screen.getByLabelText("Message") as HTMLTextAreaElement
      fireEvent.change(box, { target: { value: "look at " } })

      const text = long(5000)
      const event = await paste(box, [textItem(text)], text)

      expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
      expect((uploadDroppedFile.mock.calls[0][0] as File).name).toMatch(
        /^pasted-\d{4}-\d{2}-\d{2}-\d{6}\.txt$/,
      )
      // The PATH joined the draft. Nothing was sent: a paste must not fire a
      // half-written message.
      expect(box.value).toBe("look at /tmp/p1/.dux/uploads/pasted-x.txt ")
      expect(sentToSocket()).toEqual([])
      expect(TermStub.pastes).toEqual([])
      expect(event.defaultPrevented).toBe(true)
      // And the report says where the path went, in the compose bar's own
      // words, instead of claiming the text was typed at the agent.
      const message = vi.mocked(toast.success).mock.calls.at(-1)![0] as string
      expect(message).toContain("5000 characters")
      expect(message).toContain("added its path to your message")
      expect(message).not.toContain("typing it into the agent")
    } finally {
      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        value: 1024,
      })
      pointerStub.restore()
    }
  })

  it("strands a long paste, with its path, when the message box closes mid-upload", async () => {
    // The compose sink's liveness check still applies to a filed-away paste:
    // the bar can go away while the upload is in flight, and reporting "added
    // its path to your message" with no message box on screen would be a lie
    // about where the text went.
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 420 })
    const pointerStub = stubCoarsePointer()
    try {
      let release: (v: unknown) => void = () => {}
      uploadDroppedFile.mockReturnValue(
        new Promise((resolve) => {
          release = resolve
        }),
      )
      const view = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const box = screen.getByLabelText("Message") as HTMLTextAreaElement
      const text = long(5000)
      await paste(box, [textItem(text)], text)

      mockState = {
        ...mockState,
        bootstrap: { ...mockState.bootstrap!, compose_bar: "never" },
      }
      await act(async () => {
        view.rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      })
      await act(async () => {
        release(saved("pasted-x.txt"))
        for (let i = 0; i < 4; i++) await Promise.resolve()
      })

      expect(sentToSocket()).toEqual([])
      const message = vi.mocked(toast.warning).mock.calls.at(-1)![0] as string
      expect(message).toContain("5000 characters")
      expect(message).toContain("/tmp/p1/.dux/uploads/pasted-x.txt")
      expect(message).toContain("message box closed")
      // And the recovery, because the text reached neither the draft nor the
      // agent.
      expect(message).toContain("still on the clipboard")
    } finally {
      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        value: 1024,
      })
      pointerStub.restore()
    }
  })
})
