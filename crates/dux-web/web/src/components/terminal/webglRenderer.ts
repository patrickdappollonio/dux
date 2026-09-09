// The renderer ladder for the web terminal: WebGL first, the DOM renderer
// underneath it, always.
//
// WebGL is what makes box-drawing and block glyphs (U+2500-U+259F) tile: the DOM
// renderer lays a real font glyph into a cell of fractional width and ceil'd
// height, leaving a hairline seam at every boundary, while the webgl renderer's
// `customGlyphs` path rasterizes them itself as integer-snapped rectangles
// filling the cell. No line-height or letter-spacing tuning substitutes.
//
// It replaces the paint path and nothing else: the touch selection, the link hit
// test, the forwarded touch gestures and the viewer-report suppression all go
// through xterm's public API, `.xterm-screen` or the parser, never the painted
// output, so none of them reads xterm's per-row spans.
//
// The fallback ladder, in order:
//  1. No WebGL2 context available: the addon is never loaded.
//  2. The addon throws on activation: caught, and the DOM path is kept.
//  3. The context is lost at runtime: the addon is disposed and xterm falls back
//     on its own, so a loss costs a repaint and nothing else.
//
// A failure at either of the last two is remembered for the whole page: both say
// this browser's GL is wrong, and a remounting pane would walk back into it.
import { WebglAddon } from "@xterm/addon-webgl"
import type { Terminal } from "@xterm/xterm"

export type RendererChoice =
  | { renderer: "webgl" }
  | { renderer: "dom"; reason: "no-webgl2" | "gl-gave-up" }

/// What the decision is made from. Both values are gathered by the caller so
/// this stays pure and testable without a GL context.
export type RendererEnv = {
  /// Whether a WebGL2 context could be created at all.
  webgl2: boolean
  /// Whether GL has already failed on this page (an activation throw or a lost
  /// context). Sticky: see the module doc.
  glGaveUp: boolean
}

/// THE ONE DECISION. Pure on purpose: jsdom has no WebGL of any kind, so the
/// only part of this ladder a unit test can exercise is the choice itself.
export function chooseTerminalRenderer(env: RendererEnv): RendererChoice {
  if (env.glGaveUp) return { renderer: "dom", reason: "gl-gave-up" }
  if (!env.webgl2) return { renderer: "dom", reason: "no-webgl2" }
  return { renderer: "webgl" }
}

// Page-scoped, deliberately module state rather than per-pane: a GPU that reset
// under one pane will reset under the next one.
let glGaveUp = false

/// Reports that GL failed, so every later pane on this page takes the DOM path.
export function noteGlGaveUp(): void {
  glGaveUp = true
}

export function hasGlGivenUp(): boolean {
  return glGaveUp
}

/// Test-only reset for the page-scoped flag above.
export function resetGlGaveUpForTests(): void {
  glGaveUp = false
}

/// Probes for a WebGL2 context on a throwaway canvas and releases it at once:
/// browsers cap live GL contexts per page and drop the oldest, so a probe that
/// kept one would evict a terminal's. Any throw reads as no WebGL2.
export function detectWebgl2(): boolean {
  try {
    const canvas = document.createElement("canvas")
    const gl = canvas.getContext("webgl2")
    if (!gl) return false
    gl.getExtension("WEBGL_lose_context")?.loseContext()
    return true
  } catch {
    return false
  }
}

type Disposable = { dispose: () => void }

/// The slice of the addon the context-loss wiring needs, named so the wiring can
/// be tested against a stand-in: a real `WebglAddon` needs a real GL context.
export type ContextLossSource = Disposable & {
  onContextLoss: (listener: () => void) => unknown
}

/// A lost context costs a repaint: the addon is disposed and xterm falls back to
/// its DOM renderer, leaving the terminal and socket untouched. GL is marked as
/// given up so no pane re-creates the renderer: a context taken once will be
/// taken again, and a loop of context churn is worse than a seam.
export function wireContextLoss(
  addon: ContextLossSource,
  onLoss?: () => void,
): void {
  addon.onContextLoss(() => {
    noteGlGaveUp()
    addon.dispose()
    onLoss?.()
    console.warn(
      "dux: the terminal's WebGL context was lost; falling back to the DOM renderer",
    )
  })
}

/// Promote the terminal so its device-pixel box stops moving under it.
///
/// The webgl addon watches its canvas with a `device-pixel-content-box`
/// ResizeObserver and answers every callback by assigning `canvas.width` and
/// `canvas.height`, which clears the GL drawing buffer and costs a blank frame.
/// That box is snapped to the device pixel grid, so it depends on where the
/// canvas is and not only on how big it is: at a fractional device pixel ratio,
/// a canvas of fixed CSS size sliding across the screen reports a box that flips
/// by a pixel and back for the length of the slide. Promoting it to a
/// compositing layer snaps against the layer instead, and the box holds still.
///
/// The layout gesture (`lib/layoutGesture.ts`) cannot answer this: it parks
/// dux's own refits, and the churn is the addon reacting to the canvas moving.
///
/// Applied for as long as the renderer is attached rather than per gesture:
/// applying it at a gesture's start re-snaps the box once, costing the very
/// blank frame it exists to remove, and a divider drag, a sidebar collapse and a
/// rotation move a terminal too. The DOM renderer never gets it, so a browser on
/// that rung keeps subpixel-antialiased text.
export function pinDevicePixelBox(element: HTMLElement): void {
  element.style.willChange = "transform"
}

/// Hands the layer back when the renderer goes, so a terminal no longer painting
/// with GL is not left holding a layer for a canvas that no longer exists.
export function releaseDevicePixelBox(element: HTMLElement): void {
  element.style.removeProperty("will-change")
}

/// Loads the webgl renderer over an already-open terminal, or returns null when
/// the ladder above says to stay on the DOM renderer. The returned handle is
/// disposed by the pane's teardown; disposing it twice is harmless, and
/// disposing the Terminal releases the addon anyway, so this is belt and braces.
///
/// The addon is imported statically, inside the already-lazy terminal chunk (see
/// `LazyTerminalPane`): a dynamic import would make this function async, and an
/// async attach has to be raced against a pane that unmounted mid-flight.
export function attachWebglRenderer(
  term: Terminal,
  container: HTMLElement,
): Disposable | null {
  const choice = chooseTerminalRenderer({
    webgl2: detectWebgl2(),
    glGaveUp: hasGlGivenUp(),
  })
  if (choice.renderer === "dom") return null

  try {
    const addon = new WebglAddon()
    // Wired BEFORE activation, so a context lost during activation itself is
    // still heard.
    wireContextLoss(addon, () => releaseDevicePixelBox(container))
    term.loadAddon(addon)
    pinDevicePixelBox(container)
    return {
      dispose: () => {
        releaseDevicePixelBox(container)
        addon.dispose()
      },
    }
  } catch {
    noteGlGaveUp()
    console.warn(
      "dux: the terminal's WebGL renderer could not start; using the DOM renderer",
    )
    return null
  }
}
