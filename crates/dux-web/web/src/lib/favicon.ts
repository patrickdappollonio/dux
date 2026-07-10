// Per-instance favicon support. The operator sets `config.server.favicon`; this
// module turns that value into the favicon the browser should show:
//
//   - unset ("")            → the bundled full-colour brand duck (`/favicon.png`)
//   - a curated colour name → the brand-duck cutout SILHOUETTE filled with that
//                             colour, rendered as an inline SVG `data:` URI (so
//                             any curated colour works with no extra assets)
//
// Only a curated colour name is accepted. Legacy hex values, custom URLs, and
// dropped colour names all degrade gracefully to the default duck (with a
// one-time migration notice — see `applyFavicon`). The colour that reaches the
// generated SVG is always a validated `#rrggbb` from the map below, so nothing
// untrusted is ever interpolated into the markup.

import { toast } from "sonner"

// The brand-duck cutout path: a visually-validated asset traced from
// `public/dux-logo.png` by `scripts/gen-duck-favicon.mjs`. Even-odd fill so the
// beak/eyes/bowtie read as negative-space cutouts. Regeneration is a MANUAL step
// (potrace/jimp are not project deps): `npm i --no-save jimp potrace` then
// `node scripts/gen-duck-favicon.mjs`. potrace output is NONDETERMINISTIC, so a
// re-run yields a different-but-equivalent path — validate the duck visually
// before committing. `duckPath.test.ts` only guards against gross corruption
// (truncation, breakout characters, lost sub-shapes), NOT staleness or an exact
// match against fresh generator output.
const DUCK_PATH =
  "M 276 7.166 C 246.156 12.241, 222.096 24.737, 200.421 46.418 C 163.864 82.985, 155.245 125.298, 173.210 180 C 176.891 191.207, 190.207 217.956, 198.102 230 C 206.634 243.016, 206.547 242.789, 203.981 245.520 C 201.263 248.413, 201.681 249.099, 208.560 253.034 C 216.915 257.815, 246.088 266.675, 247.405 264.833 C 247.536 264.650, 248.623 261.163, 249.821 257.084 C 252.476 248.043, 251.774 248.672, 257.628 250.092 C 269.522 252.976, 284.102 262.216, 287.870 269.257 C 289.107 271.569, 289.489 271.676, 293.350 270.783 C 295.633 270.255, 299.823 270.052, 302.662 270.331 L 307.824 270.837 311.957 265.936 C 318.877 257.729, 330.736 251.053, 341.156 249.496 L 345.812 248.801 347.047 254.320 C 348.952 262.830, 348.528 262.756, 360.608 256.689 C 384.411 244.736, 384.342 244.792, 393.781 229.828 C 441.908 153.526, 439.003 88.744, 385.259 39.829 C 356.042 13.236, 314.075 0.690, 276 7.166 M 368.119 110.709 C 359.720 114.677, 351.823 114.901, 341.481 111.465 C 336.825 109.918, 332.731 108.936, 332.384 109.283 C 329.464 112.203, 340.995 117.148, 352.186 117.775 C 362.931 118.377, 377 113.653, 377 109.443 C 377 107.396, 374.324 107.777, 368.119 110.709 M 246.387 112.166 C 238.210 116.023, 229.609 116.812, 220.998 114.493 C 210.165 111.576, 208.751 111.467, 209.158 113.580 C 209.812 116.980, 221.059 120.086, 231.500 119.750 C 243.499 119.363, 257.659 113.336, 253.762 110.273 C 252.883 109.582, 250.646 110.156, 246.387 112.166 M 291.500 141.762 C 274.315 143.533, 256.895 151.691, 249.079 161.626 C 242.653 169.796, 231.413 175.879, 212.141 181.616 C 210.558 182.088, 211.217 187.004, 213.694 193.192 C 218.189 204.428, 252.854 224.623, 277.909 230.603 C 308.736 237.961, 352.659 223.553, 375.728 198.517 C 387.358 185.896, 387.509 184.433, 377.521 181.155 C 369.733 178.598, 363.831 174.978, 347.826 162.942 C 327.333 147.531, 307.582 140.104, 291.500 141.762 M 283.344 146.976 C 272.919 149.203, 263.816 154.133, 255.537 162.037 C 245.061 172.038, 236.627 177.663, 226.266 181.561 L 217.856 184.724 222.678 188.165 C 229.558 193.074, 232.529 194.138, 241 194.727 C 251.172 195.433, 258.067 193.515, 276.056 184.973 C 295.025 175.966, 296.311 175.913, 313.289 183.426 C 333.509 192.374, 342.650 193.631, 360.901 189.972 C 377.473 186.650, 377.585 186.552, 369.402 182.596 C 365.606 180.760, 356.672 174.892, 349.548 169.554 C 325.435 151.487, 315.622 147.091, 298 146.462 C 292.225 146.256, 285.630 146.487, 283.344 146.976 M 9.540 151.750 C 13.809 158.383, 44.677 201.320, 58.964 220.500 C 72.570 238.765, 87.904 259.860, 97.336 273.287 C 101.492 279.203, 102.118 281.386, 100.372 283.879 C 96.898 288.839, 94.527 286.580, 80.909 265.340 L 73.318 253.500 62.880 250.662 C 49.569 247.044, 46.618 247.263, 46.175 251.906 C 45.305 261.026, 62.555 283.259, 77.127 291.798 C 88.830 298.656, 117.802 307.881, 137.844 311.133 C 144.876 312.273, 147.505 313.961, 145.383 315.971 C 144.341 316.957, 139.742 316.229, 123.500 312.504 C 116.900 310.990, 110.326 309.540, 108.891 309.283 C 106.466 308.847, 105.956 309.475, 101.682 318.157 C 97.480 326.694, 95.923 331.679, 93.226 345.236 C 85.680 383.165, 110.978 464.939, 138.212 490.648 C 147.410 499.332, 156.304 502.862, 175.500 505.450 C 189.250 507.304, 407.614 506.994, 419.252 505.104 C 446.720 500.644, 458.967 491.114, 472.804 463.430 C 497.640 413.743, 505.946 362.761, 494.868 328 C 491.789 318.338, 484.522 306.601, 476.213 297.870 C 463.432 284.438, 452.367 277.670, 425 266.543 C 418.675 263.971, 410.203 260.097, 406.173 257.934 C 402.143 255.770, 398.482 254, 398.036 254 C 397.590 254, 392.329 257.262, 386.344 261.250 C 380.359 265.238, 370.011 271.407, 363.348 274.961 L 351.233 281.421 350.599 287.961 C 349.611 298.142, 345.943 316.696, 344.537 318.618 C 343.180 320.474, 344.372 321.273, 317.178 300.297 C 310.141 294.869, 309.731 294.695, 306.607 295.825 C 302.213 297.414, 296.663 297.304, 292.452 295.545 L 288.969 294.089 279.234 301.592 C 273.880 305.719, 266.243 311.600, 262.262 314.660 C 258.282 317.720, 254.415 319.989, 253.669 319.703 C 252.156 319.123, 248.749 304.354, 246.866 290.206 L 245.661 281.159 242.081 280.494 C 230.927 278.423, 205.215 268.158, 191.752 260.402 L 187.005 257.666 178.252 261.761 C 173.439 264.014, 165.900 267.330, 161.500 269.130 L 153.500 272.404 157.811 273.656 C 160.182 274.345, 165.681 275.429, 170.031 276.065 C 178.116 277.247, 181.918 279.327, 178.436 280.663 C 175.817 281.668, 161.783 279.398, 152 276.388 C 135.555 271.327, 125.504 264.724, 114.494 251.750 C 107.973 244.066, 107.583 243.890, 106.150 248 C 104.740 252.043, 100.476 252.024, 93.243 247.942 C 85.111 243.352, 83.315 242.828, 81.600 244.543 C 80.310 245.833, 79.429 244.999, 74.316 237.642 C 42.956 192.517, 11.256 149.101, 9.635 149.054 C 7.838 149.002, 7.834 149.100, 9.540 151.750 M 291 182.890 C 289.625 183.342, 283.550 186.187, 277.500 189.213 C 252.534 201.701, 238.111 202.858, 222.748 193.607 L 217.035 190.167 218.441 192.885 C 221.878 199.533, 233.729 207.726, 253.413 217.063 C 266.184 223.122, 273.014 225.319, 285.684 227.447 C 306.605 230.960, 334.820 223.457, 358.046 208.204 C 364.615 203.890, 376.833 192.466, 375.860 191.547 C 375.662 191.359, 372.125 192.086, 368 193.161 C 348.604 198.215, 331.448 196.602, 312.500 187.943 C 301.256 182.805, 295.430 181.436, 291 182.890 M 458.453 323.079 C 449.353 332.468, 430.067 339.926, 407.186 342.902 C 399.275 343.932, 395.569 345.714, 394.738 348.892 C 393.857 352.264, 397.207 357.307, 402.753 360.955 L 408.006 364.409 404.556 367.955 C 397.110 375.607, 399.025 383, 408.453 383 C 410.687 383, 410.755 383.187, 409.742 386.490 C 408.356 391.004, 409.794 394.293, 414.207 396.702 C 417.011 398.233, 420.024 398.500, 434.500 398.500 C 454.173 398.500, 460.009 396.773, 473.166 387.056 C 483.700 379.277, 497.224 359.754, 494.134 356.787 C 493.035 355.732, 492.586 355.843, 491.646 357.405 C 473.486 387.579, 456.040 397.394, 425.631 394.544 C 415.234 393.570, 412.166 391.227, 414.375 385.947 C 416.272 381.414, 415.460 380.209, 409.496 378.714 C 403.090 377.108, 402.932 375.832, 408.466 370.377 C 414.467 364.461, 414.252 362.439, 407.221 358.671 C 397.161 353.280, 396.508 348.531, 405.688 347.519 C 416.760 346.299, 438.783 340.277, 447.266 336.150 C 456.396 331.708, 465.733 323.352, 464.668 320.577 C 463.761 318.214, 462.788 318.606, 458.453 323.079"

const DUCK_VIEWBOX = "0 0 512 512"

/** The bundled default favicon served from `public/` (the full-colour duck). */
export const DEFAULT_FAVICON_HREF = "/favicon.png"

// The curated tint colours: friendly name → fixed `#rrggbb`. This list MUST equal
// the Rust `CURATED_FAVICON_COLORS` in `crates/dux-core/src/wire.rs` (a
// cross-language pin keeps them in sync). NO yellow — the unset/default favicon is
// the full-colour yellow duck (`/favicon.png`), so yellow is reserved for it and
// is not a selectable tint.
export const FAVICON_COLORS: Record<string, string> = {
  violet: "#863bff",
  blue: "#3b82f6",
  sky: "#0ea5e9",
  cyan: "#06b6d4",
  teal: "#14b8a6",
  green: "#22c55e",
  amber: "#f59e0b",
  orange: "#f97316",
  red: "#ef4444",
  pink: "#ec4899",
  rose: "#f43f5e",
}

// The safe fallback fill for the defense-in-depth clamp in `duckFaviconDataUri`.
const DEFAULT_TINT = FAVICON_COLORS.violet

const HEX_RE = /^#[0-9a-f]{6}$/

// The set of valid hex fills (the curated values) — nothing else is ever
// interpolated into the generated SVG.
const VALID_FILLS = new Set(Object.values(FAVICON_COLORS))

export type FaviconResolution =
  | { kind: "default" }
  | { kind: "tinted"; color: string }

/**
 * Resolve the operator-configured `config.server.favicon` value into what the UI
 * should show. Empty/blank/missing → the bundled full-colour duck. A curated
 * colour name → the duck silhouette tinted in that colour (always a normalized
 * `#rrggbb`, so it is safe to inline into SVG). Anything else — a legacy hex, a
 * custom URL, or a dropped colour name — degrades gracefully to the default duck.
 */
export function resolveFavicon(
  raw: string | null | undefined,
): FaviconResolution {
  const value = (raw ?? "").trim().toLowerCase()
  if (value === "") return { kind: "default" }

  const named = FAVICON_COLORS[value]
  if (named) return { kind: "tinted", color: named }

  // Non-empty but not a curated name (a legacy hex/URL/dropped name) → default.
  return { kind: "default" }
}

/**
 * Build an inline SVG `data:` URI of the brand-duck silhouette filled in `color`.
 * `color` MUST already be a validated hex (as produced by {@link resolveFavicon});
 * callers should not pass untrusted input here. Defense in depth: only a hex from
 * the curated map is ever interpolated into the SVG — a mistaken/future caller
 * falls back to a safe default fill rather than letting an unsanitized string
 * break out of the `fill` attribute.
 */
export function duckFaviconDataUri(color: string): string {
  const fill = HEX_RE.test(color) && VALID_FILLS.has(color) ? color : DEFAULT_TINT
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${DUCK_VIEWBOX}">` +
    `<path fill="${fill}" fill-rule="evenodd" d="${DUCK_PATH}"/>` +
    `</svg>`
  return `data:image/svg+xml,${encodeURIComponent(svg)}`
}

/**
 * Resolve the configured favicon to the `href` to apply and whether it is an SVG
 * (so the caller can set `type="image/svg+xml"`). The default is the bundled PNG
 * (not flagged as SVG); a curated colour is an inline SVG data URI.
 */
export function faviconHref(raw: string | null | undefined): {
  href: string
  svg: boolean
} {
  const resolved = resolveFavicon(raw)
  if (resolved.kind === "tinted") {
    return { href: duckFaviconDataUri(resolved.color), svg: true }
  }
  return { href: DEFAULT_FAVICON_HREF, svg: false }
}

/**
 * Whether `raw` is a legacy favicon value that no longer applies: a non-empty
 * string that is not a curated colour name (a hex, a URL, or a dropped name).
 * Such values render the default duck; {@link applyFavicon} announces the
 * fallback once so the operator learns their setting stopped applying.
 */
export function faviconIsLegacy(raw: string | null | undefined): boolean {
  const value = (raw ?? "").trim().toLowerCase()
  if (value === "") return false
  return FAVICON_COLORS[value] === undefined
}

// Re-arm guard for the legacy-favicon migration notice. `config.changed`
// re-applies the favicon on every rename, so we suppress a repeat notice for the
// SAME bad value but re-notify when a DIFFERENT bad value appears (or after a good
// value clears it). Holds the last legacy `raw` we announced, or null when the
// current favicon is curated/empty.
let lastNoticedLegacy: string | null = null

/**
 * Apply the configured favicon to the document by replacing the `<link rel="icon">`
 * (browsers reliably pick up a fresh element). Self-guards so it is a no-op when
 * there is no real DOM (e.g. the store's Node test environment). When the value is
 * a no-longer-supported legacy setting, fires a one-time toast (per distinct bad
 * value) pointing the operator at the command palette.
 *
 * Only touches the DOM when the resolved favicon actually changed: `config.changed`
 * fires on every unrelated rename, and re-creating an identical `<link>` each time
 * makes the tab icon flash. We compare the target href/type against the current
 * `<link rel="icon">` and return early when they already match.
 */
export function applyFavicon(raw: string | null | undefined): void {
  if (typeof document === "undefined") return
  if (typeof document.createElement !== "function" || !document.head) return

  if (faviconIsLegacy(raw)) {
    if (raw !== lastNoticedLegacy) {
      lastNoticedLegacy = raw ?? null
      toast.info(
        `The configured favicon '${(raw ?? "").trim()}' is no longer supported — ` +
          `showing the default duck. Pick a color from the command palette ` +
          `(Rename this instance).`,
      )
    }
  } else {
    // A curated/empty value clears the latch so a later, DIFFERENT bad value
    // re-notifies.
    lastNoticedLegacy = null
  }

  const { href, svg } = faviconHref(raw)
  // The default duck is a PNG, the tints are inline SVG. Use the concrete MIME
  // for both (not null) so this matches the static `<link type="image/png">` in
  // index.html — otherwise the diff below never short-circuits on first load and
  // the default favicon churns once per page load.
  const type = svg ? "image/svg+xml" : "image/png"

  // Skip the remove+recreate when the resolved favicon is unchanged, so an
  // unrelated `config.changed` doesn't flash the tab icon.
  const current = document.querySelector("link[rel='icon']")
  if (
    current &&
    current.getAttribute("href") === href &&
    current.getAttribute("type") === type
  ) {
    return
  }

  setIconLink(href, type)
}

/** Replace the `<link rel="icon">` element with one pointing at `href`. Assumes
 * the caller already confirmed a real DOM and that the target differs from the
 * current icon. */
function setIconLink(href: string, type: string): void {
  document.querySelectorAll("link[rel='icon']").forEach((el) => el.remove())

  const link = document.createElement("link")
  link.setAttribute("rel", "icon")
  link.setAttribute("type", type)
  link.setAttribute("href", href)
  document.head.appendChild(link)
}

// The amber attention dot and its dark rim (for contrast against the duck).
const ATTENTION_DOT_FILL = "#f59e0b"
const ATTENTION_DOT_RIM = "#1a1a1a"

// Composed "base icon + dot" data URLs, keyed by the base href. A given base is
// composited at most once (drawing onto a canvas is the expensive part), then the
// result is reused for every later push while the count stays above zero.
const dottedFaviconCache = new Map<string, string>()
// The base href whose dotted variant we currently WANT applied, or null when the
// clean icon should show. Set synchronously so an async compose that resolves
// after a clear does not stomp the restored clean icon.
let wantedDotBase: string | null = null
// The data URL currently on the `<link>` via the attention path, so a repeat
// call with the same state doesn't touch the DOM.
let appliedDottedIcon: string | null = null

/**
 * Composite the current favicon with an amber dot in the bottom-right corner, or
 * restore the clean icon, based on `hasAttention`. Idempotent: it composes a
 * given base at most once (cached) and only touches the DOM when the shown icon
 * changes. Self-guards on the DOM (no-op under the store's Node test env), and
 * degrades gracefully where `<canvas>` is unavailable (jsdom): it simply leaves
 * the clean icon in place rather than throwing — the browser-tab count still
 * conveys the state.
 */
export function applyAttentionFavicon(
  raw: string | null | undefined,
  hasAttention: boolean,
): void {
  if (typeof document === "undefined") return
  if (typeof document.createElement !== "function" || !document.head) return

  if (!hasAttention) {
    // Restore the clean base icon and forget any dotted state.
    wantedDotBase = null
    appliedDottedIcon = null
    applyFavicon(raw)
    return
  }

  const { href } = faviconHref(raw)
  wantedDotBase = href

  const cached = dottedFaviconCache.get(href)
  if (cached) {
    if (appliedDottedIcon !== cached) {
      setIconLink(cached, "image/png")
      appliedDottedIcon = cached
    }
    return
  }

  composeFaviconWithDot(href)
    .then((composed) => {
      if (!composed) return
      dottedFaviconCache.set(href, composed)
      // Only apply if the dot is still wanted for THIS base (a clear or a base
      // change may have landed while we composited).
      if (wantedDotBase === href && appliedDottedIcon !== composed) {
        setIconLink(composed, "image/png")
        appliedDottedIcon = composed
      }
    })
    .catch(() => {
      /* leave the clean icon in place on any compositing failure */
    })
}

/** Draw the base favicon plus an amber corner dot onto a canvas and return a PNG
 * data URL, or `null` when canvas/image loading is unavailable (e.g. jsdom). */
function composeFaviconWithDot(href: string): Promise<string | null> {
  return new Promise((resolve) => {
    const size = 64
    const canvas = document.createElement("canvas")
    canvas.width = size
    canvas.height = size
    const ctx =
      typeof canvas.getContext === "function"
        ? (canvas.getContext("2d") as CanvasRenderingContext2D | null)
        : null
    if (!ctx) {
      resolve(null)
      return
    }
    const img = new Image()
    img.onload = () => {
      try {
        ctx.clearRect(0, 0, size, size)
        ctx.drawImage(img, 0, 0, size, size)
        const r = size * 0.26
        const cx = size - r - size * 0.05
        const cy = size - r - size * 0.05
        // Dark rim first for contrast against the duck, then the amber fill.
        ctx.beginPath()
        ctx.arc(cx, cy, r + size * 0.06, 0, Math.PI * 2)
        ctx.fillStyle = ATTENTION_DOT_RIM
        ctx.fill()
        ctx.beginPath()
        ctx.arc(cx, cy, r, 0, Math.PI * 2)
        ctx.fillStyle = ATTENTION_DOT_FILL
        ctx.fill()
        resolve(canvas.toDataURL("image/png"))
      } catch {
        resolve(null)
      }
    }
    img.onerror = () => resolve(null)
    img.src = href
  })
}
