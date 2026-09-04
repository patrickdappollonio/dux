// @vitest-environment jsdom
// The WebGL2 probe. jsdom cannot produce a real context, so what is pinned
// here is the probe's CONTRACT: a context means yes and is handed straight
// back, no context means no, and a browser that throws on the context type
// means no rather than a crashed pane.
import { afterEach, describe, expect, it, vi } from "vitest"

import { detectWebgl2 } from "./webglRenderer"

afterEach(() => vi.restoreAllMocks())

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
