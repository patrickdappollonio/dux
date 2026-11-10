// @vitest-environment jsdom
// The WebGL2 probe. jsdom cannot produce a real context, so what is pinned
// here is the probe's CONTRACT: a context means yes and is handed straight
// back, no context means no, and a browser that throws on the context type
// means no rather than a crashed pane.
import { afterEach, describe, expect, it, vi } from "vitest"

import type { Terminal } from "@xterm/xterm"

// The addon's own GL work needs a real canvas; what is pinned below is how two
// panes share the page-wide give-up latch, so the addon only records what was
// done to it and hands its context-loss listener to the test.
const { addons, FakeWebglAddon } = vi.hoisted(() => {
  const addons: { disposed: boolean; lose: () => void }[] = []
  class FakeWebglAddon {
    disposed = false
    lose: () => void = () => {}
    constructor() {
      addons.push(this)
    }
    onContextLoss(listener: () => void) {
      this.lose = listener
    }
    dispose() {
      this.disposed = true
    }
  }
  return { addons, FakeWebglAddon }
})
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: FakeWebglAddon }))

import {
  attachWebglRenderer,
  detectWebgl2,
  pinDevicePixelBox,
  releaseDevicePixelBox,
  resetGlGaveUpForTests,
} from "./webglRenderer"

afterEach(() => {
  resetGlGaveUpForTests()
  vi.restoreAllMocks()
  addons.length = 0
})

describe("detectWebgl2", () => {
  it("reports yes and releases the probe context immediately", () => {
    // A browser caps live GL contexts per page and evicts the oldest when the
    // cap is hit, so a probe that kept its own would eventually take a
    // terminal's away.
    const loseContext = vi.fn()
    const gl = { getExtension: vi.fn(() => ({ loseContext })) }
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
      gl as unknown as RenderingContext,
    )

    expect(detectWebgl2()).toBe(true)
    expect(gl.getExtension).toHaveBeenCalledWith("WEBGL_lose_context")
    expect(loseContext).toHaveBeenCalledTimes(1)
  })

  it("reports yes even where the release extension is missing", () => {
    const gl = { getExtension: vi.fn(() => null) }
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
      gl as unknown as RenderingContext,
    )
    expect(detectWebgl2()).toBe(true)
  })

  it("reports no when there is no WebGL2 context", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null)
    expect(detectWebgl2()).toBe(false)
  })

  it("reports no when asking for the context throws", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => {
      throw new Error("context type refused")
    })
    expect(detectWebgl2()).toBe(false)
  })
})

describe("the device-pixel pin", () => {
  it("promotes the element and hands the layer back again", () => {
    const el = document.createElement("div")
    pinDevicePixelBox(el)
    expect(el.style.willChange).toBe("transform")

    releaseDevicePixelBox(el)
    // Removed rather than set to a value: an emptied-out declaration is still
    // a declaration, and the element should end up exactly as it started.
    expect(el.style.willChange).toBe("")
    expect(el.getAttribute("style")).toBe("")
  })

  it("is idempotent, so a re-pin cannot stack up", () => {
    const el = document.createElement("div")
    pinDevicePixelBox(el)
    pinDevicePixelBox(el)
    releaseDevicePixelBox(el)
    expect(el.style.willChange).toBe("")
  })

  it("survives a release on an element that was never pinned", () => {
    const el = document.createElement("div")
    releaseDevicePixelBox(el)
    expect(el.style.willChange).toBe("")
  })
})

describe("attaching the renderer", () => {
  const fakeTerminal = () => ({ loadAddon: vi.fn() }) as unknown as Terminal

  it("leaves the DOM fallback's container untouched", () => {
    // No WebGL2 means no canvas, so there is no drawing buffer to clear and
    // nothing for a compositing layer to buy: the DOM renderer keeps its
    // subpixel-antialiased text.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null)
    const container = document.createElement("div")

    expect(attachWebglRenderer(fakeTerminal(), container)).toBeNull()
    expect(container.style.willChange).toBe("")
  })
})

// An agent and its docked terminal are two panes on one page, so two renderers
// and two GL contexts, with one give-up latch between them.
describe("two panes on one page", () => {
  const fakeTerminal = () => ({ loadAddon: vi.fn() }) as unknown as Terminal
  const withWebgl2 = () => {
    const gl = { getExtension: vi.fn(() => null) }
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
      gl as unknown as RenderingContext,
    )
  }
  const quietly = () => vi.spyOn(console, "warn").mockImplementation(() => {})

  it("starts a pane mounted after the first gave up on the DOM renderer", () => {
    withWebgl2()
    quietly()
    const first = document.createElement("div")
    expect(attachWebglRenderer(fakeTerminal(), first)).not.toBeNull()
    addons[0].lose()
    expect(first.style.willChange).toBe("")
    expect(attachWebglRenderer(fakeTerminal(), document.createElement("div"))).toBeNull()
  })

  it("leaves the other pane drawing when one loses its context", () => {
    withWebgl2()
    quietly()
    const agent = document.createElement("div")
    const dock = document.createElement("div")
    attachWebglRenderer(fakeTerminal(), agent)
    attachWebglRenderer(fakeTerminal(), dock)
    addons[0].lose()
    expect(addons[0].disposed).toBe(true)
    expect(addons[1].disposed).toBe(false)
    expect(dock.style.willChange).toBe("transform")
  })

  it("draws both on the DOM renderer where there is no WebGL2", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null)
    expect(attachWebglRenderer(fakeTerminal(), document.createElement("div"))).toBeNull()
    expect(attachWebglRenderer(fakeTerminal(), document.createElement("div"))).toBeNull()
    expect(addons).toHaveLength(0)
  })
})
