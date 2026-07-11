import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Terminal } from "@xterm/xterm"
import {
  CLIPBOARD_MIN_INTERVAL_MS,
  NOTIFY_MIN_INTERVAL_MS,
  leadingEdgeAllowed,
  osc52SetText,
  osc777Notify,
  osc99Notify,
  osc9IsProgress,
  osc9NotifyBody,
  registerAgentNotifications,
  shouldFireNotification,
  type AgentNotificationOptions,
} from "./agentNotifications"

describe("osc9 classification", () => {
  it("treats 4;<digits> as progress", () => {
    expect(osc9IsProgress("4;1;50")).toBe(true)
    expect(osc9IsProgress("4;0;0")).toBe(true)
    expect(osc9IsProgress("4;10;5")).toBe(true)
  })

  it("treats prose and non-progress as notifications", () => {
    expect(osc9IsProgress("done")).toBe(false)
    expect(osc9IsProgress("4;hello")).toBe(false)
    expect(osc9NotifyBody("Claude needs your permission")).toBe(
      "Claude needs your permission"
    )
    expect(osc9NotifyBody("4;1;50")).toBeNull()
    expect(osc9NotifyBody("")).toBeNull()
  })
})

describe("osc99 kitty notify", () => {
  it("fires for final displayable notifications", () => {
    expect(osc99Notify(";Build finished")).toEqual({ body: "Build finished" })
    expect(osc99Notify("p=title;Hi")).toEqual({ body: "Hi" })
    expect(osc99Notify("d=1:p=body;Details")).toEqual({ body: "Details" })
  })

  it("does not fire for continuations, control parts, or queries", () => {
    expect(osc99Notify("d=0;partial")).toBeNull()
    expect(osc99Notify("p=close;")).toBeNull()
    expect(osc99Notify("p=?;")).toBeNull()
  })
})

describe("osc777 notify", () => {
  it("parses title and body", () => {
    expect(osc777Notify("notify;Title;Body")).toEqual({
      title: "Title",
      body: "Body",
    })
  })
  it("returns null for non-notify", () => {
    expect(osc777Notify("something;else")).toBeNull()
  })
})

describe("osc52 clipboard", () => {
  it("decodes a SET payload", () => {
    // base64("hello") === "aGVsbG8="
    expect(osc52SetText("c;aGVsbG8=")).toBe("hello")
  })
  it("ignores a read query", () => {
    expect(osc52SetText("c;?")).toBeNull()
  })
  it("ignores a malformed payload", () => {
    expect(osc52SetText("c")).toBeNull()
    expect(osc52SetText("c;")).toBeNull()
  })
})

describe("shouldFireNotification gating", () => {
  const base = {
    enabled: true,
    permission: "granted" as NotificationPermission,
    hidden: true,
    hasFocus: false,
  }
  it("fires only when enabled, granted, and backgrounded", () => {
    expect(shouldFireNotification(base)).toBe(true)
    expect(shouldFireNotification({ ...base, enabled: false })).toBe(false)
    expect(
      shouldFireNotification({ ...base, permission: "default" })
    ).toBe(false)
    // Foregrounded (visible AND focused): suppress.
    expect(
      shouldFireNotification({ ...base, hidden: false, hasFocus: true })
    ).toBe(false)
    // Visible but unfocused still fires.
    expect(
      shouldFireNotification({ ...base, hidden: false, hasFocus: false })
    ).toBe(true)
  })
})

describe("leadingEdgeAllowed", () => {
  it("allows only after the interval elapses", () => {
    expect(leadingEdgeAllowed(0, 999, 1000)).toBe(false)
    expect(leadingEdgeAllowed(0, 1000, 1000)).toBe(true)
    expect(leadingEdgeAllowed(Number.NEGATIVE_INFINITY, 0, 1000)).toBe(true)
  })
})

// --- registerAgentNotifications: parser wiring, gating, throttles ---

interface StubTerm {
  term: Terminal
  handlers: Record<number, (data: string) => boolean | Promise<boolean>>
  disposed: number[]
}

function stubTerm(): StubTerm {
  const handlers: StubTerm["handlers"] = {}
  const disposed: number[] = []
  const term = {
    parser: {
      registerOscHandler(
        id: number,
        cb: (data: string) => boolean | Promise<boolean>,
      ) {
        handlers[id] = cb
        return { dispose: () => disposed.push(id) }
      },
    },
  } as unknown as Terminal
  return { term, handlers, disposed }
}

class FakeNotification {
  static permission: NotificationPermission = "granted"
  static instances: Array<{ title: string; opts?: NotificationOptions }> = []
  constructor(
    public title: string,
    public opts?: NotificationOptions,
  ) {
    FakeNotification.instances.push({ title, opts })
  }
}

const docState = { hidden: true, hasFocus: false }
const writeText = vi.fn<(t: string) => Promise<void>>(() => Promise.resolve())

function baseOpts(
  over: Partial<AgentNotificationOptions> = {},
): AgentNotificationOptions {
  return {
    enabled: () => true,
    title: () => "Agent",
    clipboardMode: () => "focused",
    tag: () => "tag-1",
    ...over,
  }
}

describe("registerAgentNotifications", () => {
  beforeEach(() => {
    FakeNotification.instances = []
    FakeNotification.permission = "granted"
    docState.hidden = true
    docState.hasFocus = false
    writeText.mockClear()
    vi.stubGlobal("Notification", FakeNotification)
    vi.stubGlobal("document", {
      get hidden() {
        return docState.hidden
      },
      hasFocus: () => docState.hasFocus,
    })
    vi.stubGlobal("navigator", { clipboard: { writeText } })
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it("registers handlers for OSC 9/52/99/777 and disposes them", () => {
    const { term, handlers, disposed } = stubTerm()
    const dispose = registerAgentNotifications(term, baseOpts())
    for (const id of [9, 52, 99, 777]) {
      expect(typeof handlers[id]).toBe("function")
    }
    dispose()
    for (const id of [9, 52, 99, 777]) {
      expect(disposed).toContain(id)
    }
  })

  it("does not register OSC 8 (the pane owns the hyperlink gate)", () => {
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, baseOpts())
    expect(handlers[8]).toBeUndefined()
  })

  it("OSC 9 fires when backgrounded+granted, never for progress", () => {
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, baseOpts())
    expect(handlers[9]("Build finished")).toBe(true)
    expect(FakeNotification.instances).toHaveLength(1)
    expect(FakeNotification.instances[0].opts?.tag).toBe("tag-1")
    // A progress report falls through (returns false) and never notifies.
    expect(handlers[9]("4;1;50")).toBe(false)
    expect(FakeNotification.instances).toHaveLength(1)
  })

  it("OSC 9 is suppressed when the tab is foregrounded", () => {
    const { term, handlers } = stubTerm()
    docState.hidden = false
    docState.hasFocus = true
    registerAgentNotifications(term, baseOpts())
    expect(handlers[9]("hi")).toBe(true) // still consumed
    expect(FakeNotification.instances).toHaveLength(0)
  })

  it("OSC 99 fires only for a final displayable notification", () => {
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, baseOpts())
    expect(handlers[99](";Done")).toBe(true)
    expect(FakeNotification.instances).toHaveLength(1)
    // A continuation is consumed but does not fire.
    expect(handlers[99]("d=0;partial")).toBe(true)
    expect(FakeNotification.instances).toHaveLength(1)
  })

  it("OSC 777 fires for notify, falls through otherwise", () => {
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, baseOpts())
    expect(handlers[777]("notify;T;B")).toBe(true)
    expect(FakeNotification.instances).toHaveLength(1)
    expect(handlers[777]("something;else")).toBe(false)
  })

  it("throttles repeat notifications inside the interval", () => {
    vi.useFakeTimers()
    vi.setSystemTime(0)
    const { term, handlers } = stubTerm()
    registerAgentNotifications(term, baseOpts())
    handlers[9]("first")
    handlers[9]("second") // same instant: suppressed
    expect(FakeNotification.instances).toHaveLength(1)
    vi.setSystemTime(NOTIFY_MIN_INTERVAL_MS)
    handlers[9]("third")
    expect(FakeNotification.instances).toHaveLength(2)
  })

  it("OSC 52 writes the clipboard under focused/always, never under off", () => {
    const { term, handlers } = stubTerm()
    docState.hasFocus = true
    // off: consumed, no write.
    const off = registerAgentNotifications(term, baseOpts({ clipboardMode: () => "off" }))
    expect(handlers[52]("c;aGVsbG8=")).toBe(true)
    expect(writeText).not.toHaveBeenCalled()
    off()

    // focused: writes.
    const { term: t2, handlers: h2 } = stubTerm()
    registerAgentNotifications(t2, baseOpts({ clipboardMode: () => "focused" }))
    expect(h2[52]("c;aGVsbG8=")).toBe(true)
    expect(writeText).toHaveBeenCalledWith("hello")
  })

  it("OSC 52 read query is consumed without a clipboard write", () => {
    const { term, handlers } = stubTerm()
    docState.hasFocus = true
    registerAgentNotifications(term, baseOpts())
    expect(handlers[52]("c;?")).toBe(true)
    expect(writeText).not.toHaveBeenCalled()
  })

  it("clipboard throttle is keep-last: the final value is written after the window", () => {
    vi.useFakeTimers()
    vi.setSystemTime(0)
    const { term, handlers } = stubTerm()
    docState.hasFocus = true
    registerAgentNotifications(term, baseOpts())
    handlers[52]("c;aGVsbG8=") // "hello", immediate
    handlers[52]("c;d29ybGQ=") // "world", deferred (keep-last)
    expect(writeText).toHaveBeenCalledTimes(1)
    expect(writeText).toHaveBeenLastCalledWith("hello")
    vi.advanceTimersByTime(CLIPBOARD_MIN_INTERVAL_MS)
    expect(writeText).toHaveBeenCalledTimes(2)
    expect(writeText).toHaveBeenLastCalledWith("world")
  })
})
