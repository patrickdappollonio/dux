// The renderer ladder's DECISION, which is the only half a unit test can
// reach: jsdom has no WebGL of any kind, so whether the addon actually paints
// integer-snapped block glyphs is a container-pass question and is stated as
// such in TERMINAL.md.
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  chooseTerminalRenderer,
  hasGlGivenUp,
  noteGlGaveUp,
  resetGlGaveUpForTests,
  wireContextLoss,
} from "./webglRenderer"

afterEach(() => {
  resetGlGaveUpForTests()
  vi.restoreAllMocks()
})

describe("chooseTerminalRenderer", () => {
  it("takes webgl when the browser has a WebGL2 context and GL has not failed", () => {
    expect(chooseTerminalRenderer({ webgl2: true, glGaveUp: false })).toEqual({
      renderer: "webgl",
    })
  })

  it("stays on the DOM renderer when there is no WebGL2 context", () => {
    expect(chooseTerminalRenderer({ webgl2: false, glGaveUp: false })).toEqual({
      renderer: "dom",
      reason: "no-webgl2",
    })
  })

  it("stays on the DOM renderer once GL has given up, even where WebGL2 is present", () => {
    // A lost context or a failed activation is a fact about this browser's GL,
    // not about the pane that happened to hit it: the next pane must not walk
    // back into it.
    expect(chooseTerminalRenderer({ webgl2: true, glGaveUp: true })).toEqual({
      renderer: "dom",
      reason: "gl-gave-up",
    })
  })
})

describe("the give-up flag", () => {
  it("starts clear and is page-scoped once set", () => {
    expect(hasGlGivenUp()).toBe(false)
    noteGlGaveUp()
    expect(hasGlGivenUp()).toBe(true)
    // What a context loss does: the very next pane's decision is the DOM
    // renderer, with no probe able to talk it round.
    expect(chooseTerminalRenderer({ webgl2: true, glGaveUp: hasGlGivenUp() })).toEqual(
      { renderer: "dom", reason: "gl-gave-up" },
    )
  })

  it("is not sticky across a reset, so a test never leaks its verdict", () => {
    noteGlGaveUp()
    resetGlGaveUpForTests()
    expect(hasGlGivenUp()).toBe(false)
  })
})

describe("a lost context", () => {
  function fakeAddon() {
    let listener: (() => void) | null = null
    return {
      dispose: vi.fn(),
      onContextLoss: (cb: () => void) => {
        listener = cb
        return { dispose: () => {} }
      },
      lose: () => listener?.(),
    }
  }

  it("disposes the addon and gives up on GL, without touching the terminal", () => {
    vi.spyOn(console, "warn").mockImplementation(() => {})
    const addon = fakeAddon()
    wireContextLoss(addon)
    expect(addon.dispose).not.toHaveBeenCalled()

    addon.lose()

    // The addon goes; nothing here disposes a Terminal, closes a socket or
    // unmounts a pane, which is the whole point of the fallback.
    expect(addon.dispose).toHaveBeenCalledTimes(1)
    expect(hasGlGivenUp()).toBe(true)
    expect(chooseTerminalRenderer({ webgl2: true, glGaveUp: hasGlGivenUp() })).toEqual(
      { renderer: "dom", reason: "gl-gave-up" },
    )
  })

  it("runs the caller's release, so the terminal does not keep a layer it no longer paints into", () => {
    vi.spyOn(console, "warn").mockImplementation(() => {})
    const addon = fakeAddon()
    const release = vi.fn()
    wireContextLoss(addon, release)
    expect(release).not.toHaveBeenCalled()

    addon.lose()

    // The device-pixel pin exists for the addon's canvas; once the addon is
    // gone and xterm is back on the DOM renderer, there is no canvas to keep
    // still and the promotion is pure cost.
    expect(release).toHaveBeenCalledTimes(1)
  })

  it("says what happened, because a silent renderer swap looks like a repaint bug", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const addon = fakeAddon()
    wireContextLoss(addon)
    addon.lose()
    expect(warn).toHaveBeenCalledTimes(1)
    expect(String(warn.mock.calls[0]?.[0])).toContain("DOM renderer")
  })
})
