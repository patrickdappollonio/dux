// THE RENDERER LADDER for the web terminal: WebGL first, the DOM renderer
// underneath it, always.
//
// WHY WEBGL AT ALL. xterm's DOM renderer paints every cell as a styled span, so
// a box-drawing or block glyph (U+2500-U+259F) is a real glyph from a real font
// laid into a cell whose height is ceil'd from the TEXT face's metrics while the
// block face paints taller, and whose width is fractional. The result is a
// hairline seam at every cell boundary: a solid block banner drawn by a provider
// arrives on screen as a grid of thin gaps. The webgl renderer's `customGlyphs`
// path (on by default, and deliberately left on) does not use the font for those
// code points at all. It rasterizes them itself as integer-snapped rectangles
// filling the cell exactly, so adjacent blocks share an edge with nothing
// between them. That is a structural fix rather than a nudge: no amount of
// line-height or letter-spacing tuning makes a font glyph tile a fractional cell.
//
// WHAT THIS MODULE DOES NOT CHANGE. The webgl renderer replaces the PAINT path
// and nothing else. Everything dux reads off xterm keeps working because none of
// it reads the painted output: the touch selection (`lib/termselect.ts`) drives
// xterm's public selection API, the link hit test (`lib/termlink.ts`) and the
// forwarded touch gestures (`lib/termmouse.ts`) dispatch DOM events at
// `.xterm-screen`, which the webgl canvas is a child of rather than a
// replacement for, and the viewer-report suppression
// (`lib/suppressViewerReports.ts`) sits on the parser. Nothing anywhere reads
// xterm's per-row spans.
//
// THE FALLBACK LADDER, in order:
//  1. No WebGL2 context available (an old browser, a blocklisted GPU, a
//     software-rendering flag): the addon is never loaded and the pane renders
//     exactly as it did before this module existed.
//  2. The addon throws on activation: caught, and the same DOM path is kept.
//  3. The context is LOST at runtime (a GPU driver reset, a tab the browser
//     evicted a context from): the addon is disposed and xterm falls back to
//     the DOM renderer on its own. The terminal, the socket and the pane are
//     untouched, so a context loss costs a repaint and nothing else.
//
// A rung-2 or rung-3 failure is remembered for the whole page, not just for the
// pane that hit it. Both say something is wrong with this browser's GL, and a
// pane that remounts (a target switch, a reconnect) would otherwise walk
// straight back into it and lose its context again.
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

/// Probes for a WebGL2 context on a throwaway canvas, and releases it again
/// immediately: browsers cap the number of live GL contexts per page and drop
/// the oldest when the cap is hit, so a probe that kept its context would
/// eventually evict a terminal's. Any throw (a browser that refuses the context
/// type outright) reads as "no WebGL2".
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

/// The slice of the addon the context-loss wiring needs, named so the wiring
/// can be tested against a stand-in: constructing a real `WebglAddon` needs a
/// real GL context, which is exactly what a lost-context test cannot have.
export type ContextLossSource = Disposable & {
  onContextLoss: (listener: () => void) => unknown
}

/// WHAT A LOST CONTEXT COSTS: a repaint. The addon is disposed, xterm falls
/// back to its DOM renderer on its own, and the terminal, its socket and the
/// pane around it are untouched. GL is marked as having given up, so nothing
/// re-creates the renderer here or in the next pane to mount: a context that
/// was taken once will be taken again, and a loop of context churn is worse
/// than a seam.
export function wireContextLoss(addon: ContextLossSource): void {
  addon.onContextLoss(() => {
    noteGlGaveUp()
    addon.dispose()
    console.warn(
      "dux: the terminal's WebGL context was lost; falling back to the DOM renderer",
    )
  })
}

/// Loads the webgl renderer over an already-OPEN terminal, or does nothing and
/// returns null when the ladder above says to stay on the DOM renderer. The
/// returned handle is disposed by the pane's teardown; disposing it twice is
/// harmless, and disposing the Terminal without it would also release the addon
/// (xterm disposes what `loadAddon` registered), so this is belt and braces on
/// a GPU resource rather than the only release path.
///
/// The addon is imported STATICALLY, so a browser with no WebGL2 still pays for
/// its bytes inside the already-lazy terminal chunk (see `LazyTerminalPane`).
/// That is the deliberate trade: a dynamic import would make this function
/// async, and an async attach has to be raced against a pane that unmounted
/// while the chunk was in flight. A few tens of kilobytes inside a chunk the
/// user is already downloading a terminal from is cheaper than that race.
export function attachWebglRenderer(term: Terminal): Disposable | null {
  const choice = chooseTerminalRenderer({
    webgl2: detectWebgl2(),
    glGaveUp: hasGlGivenUp(),
  })
  if (choice.renderer === "dom") return null

  try {
    const addon = new WebglAddon()
    // Wired BEFORE activation, so a context lost during activation itself is
    // still heard.
    wireContextLoss(addon)
    term.loadAddon(addon)
    return addon
  } catch {
    noteGlGaveUp()
    console.warn(
      "dux: the terminal's WebGL renderer could not start; using the DOM renderer",
    )
    return null
  }
}
