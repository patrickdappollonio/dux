// Turns the `User-Agent` the server captured for a PTY's owner into the short label
// the take-over card shows. Pure, with no `navigator` access, and parsing by an
// ordered set of substring checks rather than a UA database.
//
// An iPad in desktop mode carries no iPad token and so reads as macOS; there is no
// reliable UA-only way to tell it apart, and the miss is benign for this label.

// Detect the operating system. ORDER MATTERS: Android UA strings also contain
// "Linux", and iOS UA strings also contain "like Mac OS X", so the more specific
// token must be tested first.
function detectOs(ua: string): string | null {
  if (/Android/.test(ua)) return "Android"
  if (/iPhone|iPad|iPod/.test(ua)) return "iOS"
  if (/Windows/.test(ua)) return "Windows"
  if (/Macintosh|Mac OS X/.test(ua)) return "macOS"
  if (/Linux/.test(ua)) return "Linux"
  return null
}

// Detect the browser. Order matters: Edge and Chrome both carry a "Chrome/" token
// and Chrome and Safari both carry a "Safari/" one, so each check excludes the
// engines above it, and Edge's mobile tokens ("EdgA/", "EdgiOS/") do not contain the
// bare desktop "Edg/". Other Chromium browsers deliberately fold into "Chrome".
function detectBrowser(ua: string): string | null {
  if (/Edg(A|iOS)?\//.test(ua)) return "Edge"
  if (/Firefox\//.test(ua)) return "Firefox"
  if (/Chrome\//.test(ua) || /CriOS\//.test(ua)) return "Chrome"
  if (/Safari\//.test(ua)) return "Safari"
  return null
}

// The one participant in PTY ownership that is not a browser: the terminal UI, which
// has no `User-Agent`. Matched exactly rather than passing unrecognized input
// through, since the header is attacker-controllable and this is rendered as copy.
// Must stay in step with `TUI_DEVICE_LABEL` in dux-core's `background_serve`.
const TUI_DEVICE_LABEL = "the dux TUI"

// Compose the take-over label from a raw `User-Agent`.
//   - Empty input, or an unrecognized OS: `null`, and the caller says "another device".
//   - Known OS and browser: "{Browser} on {OS}".
//   - Known OS, unknown browser: the OS alone.
//   - The terminal UI's fixed label: itself, verbatim, since it parses as no OS.
export function deviceLabel(
  userAgent: string | null | undefined,
): string | null {
  if (!userAgent) return null
  const ua = userAgent.trim()
  if (ua === "") return null
  if (ua === TUI_DEVICE_LABEL) return ua

  const os = detectOs(ua)
  if (os === null) return null

  const browser = detectBrowser(ua)
  return browser === null ? os : `${browser} on ${os}`
}
