// Per-instance favicon support. The operator sets `config.server.favicon`; this
// module turns that free-form string into the favicon the browser should show:
// the bundled logo (default), the dux-logo OUTLINE recoloured to a chosen colour
// (rendered as an inline SVG data URI, so any colour works with no extra assets),
// or a custom favicon URL. Colour and URL inputs are validated so nothing
// untrusted is interpolated into the generated SVG or used as a script-y URL.

/** The dux-logo silhouette path, extracted verbatim from `public/favicon.svg`. */
const DUX_LOGO_PATH =
  "M25.946 44.938c-.664.845-2.021.375-2.021-.698V33.937a2.26 2.26 0 0 0-2.262-2.262H10.287c-.92 0-1.456-1.04-.92-1.788l7.48-10.471c1.07-1.497 0-3.578-1.842-3.578H1.237c-.92 0-1.456-1.04-.92-1.788L10.013.474c.214-.297.556-.474.92-.474h28.894c.92 0 1.456 1.04.92 1.788l-7.48 10.471c-1.07 1.498 0 3.579 1.842 3.579h11.377c.943 0 1.473 1.088.89 1.83L25.947 44.94z"

const DUX_LOGO_VIEWBOX = "0 0 48 46"

/** The bundled default favicon served from `public/`. */
export const DEFAULT_FAVICON_HREF = "/favicon.svg"

// Friendly colour names mapped to fixed hex values, so the generated SVG always
// contains a known-safe `stroke` and never relies on CSS keyword support. Values
// are the Tailwind 500 shades (violet is the dux brand colour) for a familiar
// palette; extend deliberately and keep every value a plain `#rrggbb`.
const FAVICON_COLORS: Record<string, string> = {
  violet: "#863bff",
  purple: "#a855f7",
  blue: "#3b82f6",
  sky: "#0ea5e9",
  cyan: "#06b6d4",
  teal: "#14b8a6",
  green: "#22c55e",
  lime: "#84cc16",
  amber: "#f59e0b",
  orange: "#f97316",
  red: "#ef4444",
  pink: "#ec4899",
  rose: "#f43f5e",
  slate: "#64748b",
  gray: "#6b7280",
  white: "#ffffff",
  black: "#000000",
}

const HEX_RE = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/

export type FaviconResolution =
  | { kind: "default" }
  | { kind: "url"; href: string }
  | { kind: "outline"; color: string }

/**
 * Resolve the operator-configured `config.server.favicon` value into what the UI
 * should show. Empty/blank/missing → the bundled logo. A hex value or a known
 * colour name → the dux-logo outline in that colour (always a normalized
 * `#...` hex, so it is safe to inline into SVG). An `http(s)://` or single-leading
 * -slash path → a custom favicon URL. Anything else (including `javascript:`,
 * `data:`, protocol-relative `//host`, or attribute-breakout attempts) falls back
 * to the default.
 */
export function resolveFavicon(
  raw: string | null | undefined,
): FaviconResolution {
  const value = (raw ?? "").trim()
  if (value === "") return { kind: "default" }

  if (HEX_RE.test(value)) return { kind: "outline", color: value.toLowerCase() }

  const named = FAVICON_COLORS[value.toLowerCase()]
  if (named) return { kind: "outline", color: named }

  // Explicit absolute http(s) URL: allowed (the operator may host the favicon
  // elsewhere, e.g. a CDN). `javascript:`/`data:`/`vbscript:` etc. never match.
  if (/^https?:\/\//i.test(value)) return { kind: "url", href: value }

  // Same-origin absolute path. Validate through the URL parser against a sentinel
  // origin and require the result to STAY same-origin, so a value the browser
  // would normalize to another host — `//host`, or `/\host` (backslashes become
  // slashes) — is rejected instead of silently loading cross-origin.
  if (value.startsWith("/")) {
    try {
      const sentinel = "https://dux.invalid"
      if (new URL(value, `${sentinel}/`).origin === sentinel) {
        return { kind: "url", href: value }
      }
    } catch {
      // Not a parseable same-origin path → fall through to default.
    }
  }

  return { kind: "default" }
}

/**
 * Build an inline SVG `data:` URI of the dux-logo outline stroked in `color`.
 * `color` MUST already be a validated hex (as produced by {@link resolveFavicon});
 * callers should not pass untrusted input here.
 */
export function outlineFaviconDataUri(color: string): string {
  // Defense in depth: only a validated hex is ever interpolated into the SVG.
  // `resolveFavicon` never produces a non-hex colour, but a future/mistaken
  // caller falls back to the brand colour rather than letting an unsanitized
  // string break out of the `stroke` attribute.
  const stroke = HEX_RE.test(color) ? color : FAVICON_COLORS.violet
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${DUX_LOGO_VIEWBOX}">` +
    `<path d="${DUX_LOGO_PATH}" fill="none" stroke="${stroke}" stroke-width="2.5" stroke-linejoin="round"/>` +
    `</svg>`
  return `data:image/svg+xml,${encodeURIComponent(svg)}`
}

/**
 * Resolve the configured favicon to the `href` to apply and whether it is an SVG
 * (so the caller can set `type="image/svg+xml"`). A custom URL is returned
 * untouched and not flagged as SVG (its real type is unknown).
 */
export function faviconHref(raw: string | null | undefined): {
  href: string
  svg: boolean
} {
  const resolved = resolveFavicon(raw)
  if (resolved.kind === "url") return { href: resolved.href, svg: false }
  if (resolved.kind === "outline") {
    return { href: outlineFaviconDataUri(resolved.color), svg: true }
  }
  return { href: DEFAULT_FAVICON_HREF, svg: true }
}

/**
 * Apply the configured favicon to the document by replacing the `<link rel="icon">`
 * (browsers reliably pick up a fresh element). Self-guards so it is a no-op when
 * there is no real DOM (e.g. the store's Node test environment).
 */
export function applyFavicon(raw: string | null | undefined): void {
  if (typeof document === "undefined") return
  if (typeof document.createElement !== "function" || !document.head) return

  const { href, svg } = faviconHref(raw)
  document.querySelectorAll("link[rel='icon']").forEach((el) => el.remove())

  const link = document.createElement("link")
  link.setAttribute("rel", "icon")
  if (svg) link.setAttribute("type", "image/svg+xml")
  link.setAttribute("href", href)
  document.head.appendChild(link)
}
