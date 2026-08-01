// Build-time diagrams: Graphviz DOT in, themed inline SVG out.
//
// WHY GRAPHVIZ-WASM AND NOT MERMAID
//
// The site ships almost no JavaScript on purpose, so a diagram renderer that
// runs in the visitor's browser was never on the table. The usual build-time
// alternative (mermaid-cli) drives a headless Chromium through Puppeteer,
// because Mermaid measures text with `getBBox()` and needs a real layout
// engine. That would put a browser download in every contributor's `npm ci`,
// and the frontend build is a hard failure in this repo, so a flaky browser
// fetch would become a broken build.
//
// `@hpcc-js/wasm-graphviz` is Graphviz compiled to WebAssembly. It lays out and
// renders in plain Node with no browser, has zero runtime dependencies, and
// emits SVG markup that goes straight into the page. Nothing is shipped to the
// visitor but the SVG.
//
// THEMING
//
// Graphviz writes colors as *presentation attributes* (`fill="lightgrey"`,
// `stroke="black"`). Presentation attributes lose to any CSS rule, so the
// styling in global.css (`.diagram …`) overrides all of them and every color in
// a rendered diagram comes from the site's own tokens. That is why the DOT
// sources below never name a color: they name a `class`, which Graphviz copies
// onto the emitted `<g>`, and CSS does the rest.
//
// The one exception is *size*: font sizes stay as Graphviz computed them,
// because the layout was measured against them. A secondary label line is
// authored at 9pt and CSS dims it via an attribute selector on that size (see
// global.css); changing a point size in DOT means changing it there too.

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
 * Widest a diagram is allowed to shrink to before its container scrolls
 * instead. Below this the labels stop being readable, and a horizontal scroll
 * inside the figure beats unreadable text on a phone.
 */
const MIN_READABLE_PX = 380;

/**
 * Turn Graphviz's standalone SVG document into an inline, responsive fragment.
 *
 * - drops the XML prolog, DOCTYPE and generator comments (an inline SVG cannot
 *   carry a prolog, and the generator comment is noise in every page)
 * - drops the per-node `<title>` elements, which are Graphviz's internal node
 *   ids and would otherwise surface as tooltips reading "cluster_host"
 * - replaces the fixed `width`/`height` in points with `width="100%"`, keeping
 *   the `viewBox` so the drawing scales, and records the intrinsic width as a
 *   custom property so CSS can cap it and set a readable floor
 *
 * Exported for the unit tests; callers want `renderDot`.
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
 * Wrap a rendered SVG in the figure chrome: a scroll container (so a diagram
 * wider than the column scrolls in its own box rather than the page body) and
 * an optional caption.
 */
export function diagramFigure(svg, caption = "", className = "") {
  const body = `<div class="diagram-scroll">${svg}</div>`;
  const figcaption = caption ? `<figcaption class="diagram-caption">${caption}</figcaption>` : "";
  // The caller's class goes on the FIGURE, which is this component's root and
  // the element that carries the sizing. It used to be applied to the `<svg>`
  // instead, so a modifier like `diagram--full` silently did nothing: the rule
  // targeting it never reached the figure's own `max-width`.
  const extra = className ? ` ${className}` : "";
  return `<figure class="diagram not-prose${extra}">${body}${figcaption}</figure>`;
}

/** Render and wrap in one step. */
export async function renderDiagram(dot, { caption = "", label = "", className = "" } = {}) {
  return diagramFigure(await renderDot(dot, { label: label || caption }), caption, className);
}
