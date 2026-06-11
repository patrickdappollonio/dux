import { afterEach, describe, expect, it, vi } from "vitest"

import { copyToClipboard } from "./clipboard"

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
