import { afterEach, describe, expect, it, vi } from "vitest"

import { copyToClipboard, watchComposition } from "./clipboard"

// The test runner is the node environment (no DOM), so stub navigator/document
// per case. These mirror the two code paths: the async Clipboard API and the
// legacy hidden-textarea + execCommand fallback.

afterEach(() => vi.unstubAllGlobals())

function fakeTextarea() {
  return {
    value: "",
    style: {} as Record<string, string>,
    focus: vi.fn(),
    select: vi.fn(),
  }
}

describe("copyToClipboard", () => {
  it("uses navigator.clipboard.writeText when available", async () => {
    const writeText = vi.fn(async () => {})
    vi.stubGlobal("navigator", { clipboard: { writeText } })
    expect(await copyToClipboard("/work/tree")).toBe(true)
    expect(writeText).toHaveBeenCalledWith("/work/tree")
  })

  it("falls back to execCommand when the Clipboard API is absent", async () => {
    vi.stubGlobal("navigator", {})
    const ta = fakeTextarea()
    const removeChild = vi.fn()
    const execCommand = vi.fn(() => true)
    vi.stubGlobal("document", {
      createElement: () => ta,
      body: { appendChild: vi.fn(), removeChild },
      execCommand,
    })
    expect(await copyToClipboard("/p")).toBe(true)
    expect(ta.value).toBe("/p")
    expect(execCommand).toHaveBeenCalledWith("copy")
    expect(removeChild).toHaveBeenCalledWith(ta)
  })

  it("falls back to execCommand when writeText rejects", async () => {
    vi.stubGlobal("navigator", {
      clipboard: {
        writeText: vi.fn(async () => {
          throw new Error("denied")
        }),
      },
    })
    const execCommand = vi.fn(() => true)
    vi.stubGlobal("document", {
      createElement: () => fakeTextarea(),
      body: { appendChild: vi.fn(), removeChild: vi.fn() },
      execCommand,
    })
    expect(await copyToClipboard("/p")).toBe(true)
    expect(execCommand).toHaveBeenCalled()
  })

  it("returns false when both paths fail", async () => {
    vi.stubGlobal("navigator", {})
    vi.stubGlobal("document", {
      createElement: () => fakeTextarea(),
      body: { appendChild: vi.fn(), removeChild: vi.fn() },
      execCommand: () => false,
    })
    expect(await copyToClipboard("/p")).toBe(false)
  })

  it("removes the textarea even if execCommand throws (finally cleanup)", async () => {
    vi.stubGlobal("navigator", {})
    const removeChild = vi.fn()
    const ta = fakeTextarea()
    vi.stubGlobal("document", {
      createElement: () => ta,
      body: { appendChild: vi.fn(), removeChild },
      execCommand: () => {
        throw new Error("boom")
      },
    })
    expect(await copyToClipboard("/p")).toBe(false)
    expect(removeChild).toHaveBeenCalledWith(ta)
  })
})

// A document whose `execCommand("copy")` dispatches a copy event to the listener
// the copy registered, the way a browser does when the page holds a user
// activation. `fires: false` is a browser that runs no copy event (no
// activation, or Safari with nothing selected).
function copyEventDocument(opts: { fires: boolean; clipboardData?: boolean }) {
  const listeners = new Set<(ev: unknown) => void>()
  const setData = vi.fn()
  const createElement = vi.fn(() => fakeTextarea())
  const doc = {
    addEventListener: vi.fn((type: string, l: (ev: unknown) => void) => {
      if (type === "copy") listeners.add(l)
    }),
    removeEventListener: vi.fn((type: string, l: (ev: unknown) => void) => {
      if (type === "copy") listeners.delete(l)
    }),
    execCommand: vi.fn(() => {
      if (!opts.fires) return false
      const ev = {
        clipboardData: opts.clipboardData === false ? null : { setData },
        preventDefault: vi.fn(),
        stopImmediatePropagation: vi.fn(),
      }
      for (const l of [...listeners]) l(ev)
      return true
    }),
    createElement,
    body: { appendChild: vi.fn(), removeChild: vi.fn() },
  }
  return { doc, setData, createElement, listeners }
}

describe("copyToClipboard without the Clipboard API", () => {
  it("writes the text from a copy event and never creates a focusable textarea", async () => {
    vi.stubGlobal("navigator", {})
    const { doc, setData, createElement, listeners } = copyEventDocument({ fires: true })
    vi.stubGlobal("document", doc)
    expect(await copyToClipboard("hello")).toBe(true)
    expect(setData).toHaveBeenCalledWith("text/plain", "hello")
    expect(createElement).not.toHaveBeenCalled()
    expect(listeners.size).toBe(0)
  })

  it("reports a copy the browser refused for want of a gesture, without the textarea", async () => {
    vi.stubGlobal("navigator", { userActivation: { isActive: false } })
    const { doc, createElement } = copyEventDocument({ fires: false })
    vi.stubGlobal("document", doc)
    expect(await copyToClipboard("hello")).toBe(false)
    expect(createElement).not.toHaveBeenCalled()
  })

  it("falls back to the textarea where a gesture runs no copy event", async () => {
    vi.stubGlobal("navigator", { userActivation: { isActive: true } })
    const { doc, createElement } = copyEventDocument({ fires: false })
    vi.stubGlobal("document", doc)
    await copyToClipboard("hello")
    expect(createElement).toHaveBeenCalledWith("textarea")
  })

  it("falls back to the textarea when the copy event carries no clipboard data", async () => {
    vi.stubGlobal("navigator", {})
    const { doc, createElement } = copyEventDocument({ fires: true, clipboardData: false })
    vi.stubGlobal("document", doc)
    await copyToClipboard("hello")
    expect(createElement).toHaveBeenCalledWith("textarea")
  })

  it("never takes focus for the textarea in the middle of an IME composition", async () => {
    const target = new EventTarget()
    const stop = watchComposition(target)
    try {
      vi.stubGlobal("navigator", { userActivation: { isActive: true } })
      const { doc, createElement } = copyEventDocument({ fires: false })
      vi.stubGlobal("document", doc)
      target.dispatchEvent(new Event("compositionstart"))
      expect(await copyToClipboard("hello")).toBe(false)
      expect(createElement).not.toHaveBeenCalled()
      target.dispatchEvent(new Event("compositionend"))
      await copyToClipboard("hello")
      expect(createElement).toHaveBeenCalledWith("textarea")
    } finally {
      stop()
    }
  })
})
