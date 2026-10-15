import type { Terminal } from "@xterm/xterm"
import type { ReactElement } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// sonner is spied where `lib/notify.ts` reaches it, the one module allowed to;
// the toasts asserted here are what notify handed it.
vi.mock("sonner", () => {
  const toast = Object.assign(vi.fn(), {
    success: vi.fn(),
    info: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
    getToasts: vi.fn(() => []),
  })
  return { toast }
})

const { toast } = await import("sonner")
const {
  blockedCopyToastId,
  closeCopyText,
  copyKeepingFocus,
  copyPreview,
  COPY_PREVIEW_CHARS,
} = await import("./agentCopy")
const agentCopy = await import("./agentCopy")
const { registerAgentNotifications } = await import("./agentNotifications")
const { watchComposition } = await import("./clipboard")

// A document with just enough of the DOM for the hidden-textarea fallback: an
// element that takes focus while it is in the page, and focus falling back to
// the body when it is removed, the way a browser does it.
interface FakeEl {
  focus: (opts?: FocusOptions) => void
  isConnected: boolean
}

function fakeDocument(opts: { hasFocus: boolean; execCommand: boolean }) {
  const body: FakeEl = { focus: () => {}, isConnected: true }
  const state = { active: body as FakeEl }
  const doc = {
    body: {
      appendChild: vi.fn(),
      removeChild: vi.fn(() => {
        state.active = body
      }),
    },
    get activeElement() {
      return state.active
    },
    hasFocus: () => opts.hasFocus,
    hidden: false,
    createElement: () => {
      const ta: FakeEl & { value: string; style: object; select: () => void } = {
        value: "",
        style: {},
        isConnected: true,
        select: () => {},
        focus: () => {
          state.active = ta
        },
      }
      return ta
    },
    execCommand: vi.fn(() => opts.execCommand),
  }
  const surface: FakeEl = {
    isConnected: true,
    focus: vi.fn(() => {
      state.active = surface
    }),
  }
  return { doc, state, surface }
}

function stubTerm() {
  const handlers: Record<number, (data: string) => boolean> = {}
  const term = {
    parser: {
      registerOscHandler(id: number, cb: (data: string) => boolean) {
        handlers[id] = cb
        return { dispose: () => {} }
      },
    },
  } as unknown as Terminal
  return { term, handlers }
}

// "hello" as an OSC 52 SET.
const HELLO = "c;aGVsbG8="

type ToastCall = [unknown, { id?: string; duration?: number; action?: ReactElement }]

function lastCall(fn: unknown): ToastCall {
  const calls = (fn as ReturnType<typeof vi.fn>).mock.calls
  return calls[calls.length - 1] as ToastCall
}

function pressAction(fn: unknown): void {
  const action = lastCall(fn)[1].action as ReactElement<{ onClick: () => void }>
  action.props.onClick()
}

// Let the copy promise and the toast it raises settle.
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

beforeEach(() => {
  vi.clearAllMocks()
  closeCopyText()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("copyPreview", () => {
  it("flattens whitespace onto one line", () => {
    expect(copyPreview("  a\n\tb  c ")).toBe("a b c")
  })

  it("cuts long text by characters, never inside one", () => {
    const text = "é".repeat(COPY_PREVIEW_CHARS + 5)
    expect(copyPreview(text)).toBe(`${"é".repeat(COPY_PREVIEW_CHARS)}…`)
  })
})

describe("an agent's clipboard copy", () => {
  it("lands through the clipboard API in a secure context, with no toast", async () => {
    const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
    const writeText = vi.fn(async () => {})
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", { clipboard: { writeText } })
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, { enabled: () => false, title: () => "Agent" })
    handlers[52](HELLO)
    await settle()
    expect(writeText).toHaveBeenCalledWith("hello")
    expect(doc.execCommand).not.toHaveBeenCalled()
    expect(toast.warning).not.toHaveBeenCalled()
  })

  it("lands over plain HTTP through a copy event, never moving focus", async () => {
    const { doc, state, surface } = fakeDocument({ hasFocus: true, execCommand: true })
    const listeners = new Set<(ev: unknown) => void>()
    const setData = vi.fn()
    const createElement = vi.spyOn(doc, "createElement")
    Object.assign(doc, {
      addEventListener: (type: string, l: (ev: unknown) => void) => {
        if (type === "copy") listeners.add(l)
      },
      removeEventListener: (type: string, l: (ev: unknown) => void) => {
        if (type === "copy") listeners.delete(l)
      },
    })
    doc.execCommand.mockImplementation(() => {
      const ev = { clipboardData: { setData }, preventDefault() {}, stopImmediatePropagation() {} }
      for (const l of [...listeners]) l(ev)
      return true
    })
    state.active = surface
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", {})
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, { enabled: () => false, title: () => "Agent" })
    handlers[52](HELLO)
    await settle()
    expect(setData).toHaveBeenCalledWith("text/plain", "hello")
    // No element was created to take focus, and focus never left the surface.
    expect(createElement).not.toHaveBeenCalled()
    expect(surface.focus).not.toHaveBeenCalled()
    expect(state.active).toBe(surface)
    expect(toast.warning).not.toHaveBeenCalled()
  })

  it("keeps the text in the toast instead of taking focus from an IME composition", async () => {
    const target = new EventTarget()
    const stop = watchComposition(target)
    try {
      const { doc, state, surface } = fakeDocument({ hasFocus: true, execCommand: true })
      state.active = surface
      const createElement = vi.spyOn(doc, "createElement")
      vi.stubGlobal("document", doc)
      vi.stubGlobal("navigator", {})
      target.dispatchEvent(new Event("compositionstart"))
      const { term, handlers } = stubTerm()
      registerAgentNotifications(term, {
        enabled: () => false,
        title: () => "fix-login",
        tag: () => "dux-agent-7",
      })
      handlers[52](HELLO)
      await settle()
      expect(createElement).not.toHaveBeenCalled()
      expect(state.active).toBe(surface)
      expect(toast.warning).toHaveBeenCalledTimes(1)
      expect(lastCall(toast.warning)[1].id).toBe(blockedCopyToastId("dux-agent-7"))
    } finally {
      stop()
    }
  })

  it("falls back to the hidden textarea over plain HTTP and hands focus back", async () => {
    const { doc, state, surface } = fakeDocument({ hasFocus: true, execCommand: true })
    state.active = surface
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", {})
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, { enabled: () => false, title: () => "Agent" })
    handlers[52](HELLO)
    await settle()
    expect(doc.execCommand).toHaveBeenCalledWith("copy")
    expect(surface.focus).toHaveBeenCalledWith({ preventScroll: true })
    expect(state.active).toBe(surface)
    expect(toast.warning).not.toHaveBeenCalled()
  })

  it("hands focus back even when the fallback is refused", async () => {
    const { doc, state, surface } = fakeDocument({ hasFocus: true, execCommand: false })
    state.active = surface
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", {})
    expect(await copyKeepingFocus("x")).toBe(false)
    expect(state.active).toBe(surface)
  })

  it("leaves focus where the user moved it while the clipboard API was working", async () => {
    const { doc, state, surface } = fakeDocument({ hasFocus: true, execCommand: false })
    state.active = surface
    let finish = () => {}
    const writeText = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", { clipboard: { writeText } })
    const copied = copyKeepingFocus("x")
    const elsewhere: FakeEl = { focus: () => {}, isConnected: true }
    state.active = elsewhere
    finish()
    expect(await copied).toBe(true)
    expect(state.active).toBe(elsewhere)
    expect(surface.focus).not.toHaveBeenCalled()
  })

  it("raises one sticky toast naming the pane, with a Copy button, when both paths fail", async () => {
    const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", {})
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, {
      enabled: () => false,
      title: () => "fix-login",
      tag: () => "dux-agent-7",
    })
    handlers[52](HELLO)
    await settle()
    expect(toast.warning).toHaveBeenCalledTimes(1)
    const [, options] = lastCall(toast.warning)
    expect(options.id).toBe(blockedCopyToastId("dux-agent-7"))
    expect(options.duration).toBe(Infinity)
    expect((options.action as ReactElement<{ children: string }>).props.children).toBe("Copy")
  })

  it("raises the blocked-copy toast only in the tab that owns the pane's input", async () => {
    for (const ownsInput of [true, false]) {
      vi.clearAllMocks()
      const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
      vi.stubGlobal("document", doc)
      vi.stubGlobal("navigator", {})
      const { term, handlers } = stubTerm()
      registerAgentNotifications(term, {
        enabled: () => false,
        title: () => "fix-login",
        tag: () => "dux-agent-7",
        ownsInput: () => ownsInput,
      })
      handlers[52](HELLO)
      await settle()
      // A watching tab still tries the write; only the toast is the owner's.
      expect(doc.execCommand).toHaveBeenCalledWith("copy")
      expect(toast.warning).toHaveBeenCalledTimes(ownsInput ? 1 : 0)
    }
  })

  it("never writes under mode off, or while the window is unfocused", async () => {
    for (const [mode, hasFocus] of [
      ["off", true],
      ["focused", false],
      ["always", false],
    ] as const) {
      const { doc } = fakeDocument({ hasFocus, execCommand: true })
      const writeText = vi.fn(async () => {})
      vi.stubGlobal("document", doc)
      vi.stubGlobal("navigator", { clipboard: { writeText } })
      const { term, handlers } = stubTerm()
      registerAgentNotifications(term, {
        enabled: () => false,
        title: () => "Agent",
        clipboardMode: () => mode,
      })
      expect(handlers[52](HELLO)).toBe(true)
      await settle()
      expect(writeText).not.toHaveBeenCalled()
      expect(doc.execCommand).not.toHaveBeenCalled()
      expect(toast.warning).not.toHaveBeenCalled()
    }
  })
})

describe("the blocked-copy toast", () => {
  it("quotes a preview and names the pane as chips", async () => {
    const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", {})
    agentCopy.reportBlockedCopy("p1", "fix-login", "line one\nline two")
    const [body] = lastCall(toast.warning)
    // notify renders Prose into a fragment of strings and chips.
    const children = (body as ReactElement<{ children: unknown[] }>).props.children
    const chips = children
      .filter((c): c is ReactElement<{ children: string }> => typeof c === "object")
      .map((c) => c.props.children)
    expect(chips).toEqual(["fix-login", "line one line two"])
  })

  it("Copy copies the full text it kept, and says so on the same toast", async () => {
    const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
    vi.stubGlobal("document", doc)
    const writeText = vi.fn(async () => {})
    vi.stubGlobal("navigator", { clipboard: { writeText } })
    const full = "x".repeat(COPY_PREVIEW_CHARS * 3)
    agentCopy.reportBlockedCopy("p1", "fix-login", full)
    pressAction(toast.warning)
    await settle()
    expect(writeText).toHaveBeenCalledWith(full)
    const [, options] = lastCall(toast.success)
    expect(options.id).toBe(blockedCopyToastId("p1"))
    // The replacement clears the button rather than inheriting it.
    expect("action" in options && options.action === undefined).toBe(true)
  })

  it("a refused Copy press offers the text to select, keeping it", async () => {
    const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
    vi.stubGlobal("document", doc)
    vi.stubGlobal("navigator", {})
    agentCopy.reportBlockedCopy("p1", "fix-login", "secret text")
    pressAction(toast.warning)
    await settle()
    const [, options] = lastCall(toast.error)
    expect(options.id).toBe(blockedCopyToastId("p1"))
    expect(options.duration).toBe(Infinity)
    const action = options.action as ReactElement<{ children: string }>
    expect(action.props.children).toBe("Show text")

    expect(agentCopy.shownCopyText()).toBeNull()
    pressAction(toast.error)
    expect(agentCopy.shownCopyText()).toEqual({ source: "fix-login", text: "secret text" })
  })

  it("an older Copy press finishing late leaves a newer blocked copy's toast alone", async () => {
    const { doc } = fakeDocument({ hasFocus: true, execCommand: false })
    vi.stubGlobal("document", doc)
    let finish = () => {}
    const writeText = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    vi.stubGlobal("navigator", { clipboard: { writeText } })
    agentCopy.reportBlockedCopy("p1", "fix-login", "first")
    pressAction(toast.warning)
    agentCopy.reportBlockedCopy("p1", "fix-login", "second")
    finish()
    await settle()
    expect(toast.success).not.toHaveBeenCalled()
    expect(toast.error).not.toHaveBeenCalled()
    // The newer toast's own Copy still works and still reports on its id.
    pressAction(toast.warning)
    finish()
    await settle()
    expect(writeText).toHaveBeenLastCalledWith("second")
    expect(lastCall(toast.success)[1].id).toBe(blockedCopyToastId("p1"))
  })

  it("a newer blocked copy from the same pane replaces the older toast", () => {
    agentCopy.reportBlockedCopy("p1", "fix-login", "first")
    agentCopy.reportBlockedCopy("p1", "fix-login", "second")
    agentCopy.reportBlockedCopy("p2", "other", "third")
    const ids = (toast.warning as ReturnType<typeof vi.fn>).mock.calls.map(
      (c) => (c[1] as { id: string }).id,
    )
    expect(ids).toEqual([
      blockedCopyToastId("p1"),
      blockedCopyToastId("p1"),
      blockedCopyToastId("p2"),
    ])
  })
})
