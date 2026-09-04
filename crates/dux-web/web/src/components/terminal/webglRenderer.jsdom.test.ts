// @vitest-environment jsdom
// The WebGL2 probe. jsdom cannot produce a real context, so what is pinned
// here is the probe's CONTRACT: a context means yes and is handed straight
// back, no context means no, and a browser that throws on the context type
// means no rather than a crashed pane.
import { afterEach, describe, expect, it, vi } from "vitest"

import type { Terminal } from "@xterm/xterm"

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
