// Build-time diagrams: Graphviz DOT in, themed inline SVG out. Graphviz compiled
// to WebAssembly lays out in plain Node with no browser, which mermaid-cli would
// require: it measures text with `getBBox()` and drives a headless Chromium, and
// a browser download in `npm ci` would make a flaky fetch a broken build.
//
// DOT sources never name a color, only a `class` Graphviz copies onto the
// emitted `<g>`: Graphviz writes colors as presentation attributes, which lose
// to global.css's `.diagram` rules, so every color comes from the site's tokens.
// Font sizes are the exception and stay as Graphviz computed them, because the
// layout was measured against them; a 9pt secondary label is dimmed by an
// attribute selector on that size, so changing it in DOT means changing it there.

import { Graphviz } from "@hpcc-js/wasm-graphviz";

/** The WASM module is a few MB; load it once per build, not once per diagram. */
let graphvizPromise;
function loadGraphviz() {
  graphvizPromise ??= Graphviz.load();
  return graphvizPromise;
}

/** Graphviz reports its canvas in points; browsers lay out in CSS pixels. */
const PX_PER_PT = 96 / 72;

/**
 * Widest a diagram may shrink to before its container scrolls instead: below
 * this the labels stop being readable.
 */
const MIN_READABLE_PX = 380;

/**
 * Turn Graphviz's standalone SVG document into an inline, responsive fragment:
 * the prolog and DOCTYPE go (an inline SVG cannot carry one), the per-node
 * `<title>` elements go (they are internal node ids and surface as tooltips),
 * and the fixed width/height in points become `width="100%"` plus the intrinsic
 * width as a custom property. Exported for the tests; callers want `renderDot`.
 */
export function inlineSvg(svg, { className = "", label = "" } = {}) {
  const start = svg.indexOf("<svg");
  if (start < 0) throw new Error("graphviz returned no <svg> element");
  let out = svg.slice(start);

  out = out.replace(/<title>[\s\S]*?<\/title>/g, "");

  const viewBox = /viewBox="([\d.\-\s]+)"/.exec(out);
  if (!viewBox) throw new Error("graphviz returned an <svg> with no viewBox");
  const widthPt = Number(viewBox[1].trim().split(/\s+/)[2]);
  const widthPx = Math.round(widthPt * PX_PER_PT);
  const minPx = Math.min(widthPx, MIN_READABLE_PX);

  const attrs = [
    'width="100%"',
    'preserveAspectRatio="xMidYMid meet"',
    `class="diagram-svg${className ? ` ${className}` : ""}"`,
    label ? `role="img" aria-label="${escapeAttr(label)}"` : 'role="presentation"',
    `style="--diagram-width:${widthPx}px;--diagram-min-width:${minPx}px"`,
  ].join(" ");

  return out.replace(/<svg[^>]*?(viewBox="[^"]*")[^>]*>/, `<svg $1 ${attrs}>`);
}

function escapeAttr(value) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/**
 * Lay out a DOT source and return an inline SVG string.
 *
 * `label` becomes the SVG's accessible name. Without one the drawing is marked
 * `role="presentation"`, which is the honest answer when the surrounding prose
 * already says everything the picture does.
 */
export async function renderDot(dot, options = {}) {
  const graphviz = await loadGraphviz();
  return inlineSvg(graphviz.layout(dot, "svg", "dot"), options);
}

/**
 * Wrap a rendered SVG in the figure chrome: a scroll container, so a diagram
 * wider than the column scrolls in its own box rather than the page body.
 */
export function diagramFigure(svg, caption = "", className = "") {
  const body = `<div class="diagram-scroll">${svg}</div>`;
  const figcaption = caption ? `<figcaption class="diagram-caption">${caption}</figcaption>` : "";
  // The caller's class goes on the figure, not the `<svg>`: the figure is the
  // root and the element that carries the sizing a modifier needs to reach.
  const extra = className ? ` ${className}` : "";
  return `<figure class="diagram not-prose${extra}">${body}${figcaption}</figure>`;
}

/** Render and wrap in one step. */
export async function renderDiagram(dot, { caption = "", label = "", className = "" } = {}) {
  return diagramFigure(await renderDot(dot, { label: label || caption }), caption, className);
}
