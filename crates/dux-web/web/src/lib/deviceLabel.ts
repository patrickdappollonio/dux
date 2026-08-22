// Turn a raw `User-Agent` string into a short, human take-over label such as
// "Chrome on macOS". The other device that owns a PTY is only known to the server,
// which captures its `User-Agent` and ships it on the `pty.owner` handover; this
// pure helper is where that string becomes the copy the take-over modal shows.
//
// Kept deliberately pure (string in, string-or-null out) with NO DOM/`navigator`
// access so it is fully unit-testable and can run anywhere. The parsing is a small,
// ordered set of substring checks — enough to name the common browser/OS pairs, not
// a full UA database.
//
// Known limitation: an iPad in desktop mode reports itself as "Macintosh" (no
// iPad/iPhone token), so it reads here as macOS. There is no reliable UA-only way to
// distinguish that case, and naming it macOS is a benign miss for this label.

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

// Detect the browser. ORDER MATTERS: Edge and Chrome both carry a "Chrome/" token,
// and Chrome and Safari both carry a "Safari/" token, so each check excludes the
// engines layered above it. Chromium-based browsers (Opera, Brave, Vivaldi, …) are
// intentionally folded into "Chrome" per the product spec.
// Edge uses distinct tokens per platform: desktop is "Edg/", Android is "EdgA/",
// and iOS is "EdgiOS/" — none of the mobile variants contain the bare "Edg/", so
// they would otherwise fall through to the Chrome check below.
function detectBrowser(ua: string): string | null {
  if (/Edg(A|iOS)?\//.test(ua)) return "Edge"
  if (/Firefox\//.test(ua)) return "Firefox"
  if (/Chrome\//.test(ua) || /CriOS\//.test(ua)) return "Chrome"
  if (/Safari\//.test(ua)) return "Safari"
  return null
}

// The one participant in PTY ownership that is not a browser: the dux terminal
// UI, which records a fixed label instead of a `User-Agent` because it has none.
// Matched EXACTLY, and only this one string, rather than passing unrecognized
// input through: the `User-Agent` is attacker-controllable and this value is
// rendered as copy, so "anything I cannot parse is a device name" would turn the
// take-over card into an echo of whatever a client sent. Kept in step with
// `TUI_DEVICE_LABEL` in `dux-core`'s `background_serve` module.
const TUI_DEVICE_LABEL = "the dux TUI"

// Compose the take-over label from a raw `User-Agent`.
//   - Empty/null/whitespace input, or an unrecognized OS → `null` (the caller shows
//     a generic "another device" fallback).
//   - Known OS + known browser → "{Browser} on {OS}" (e.g. "Chrome on macOS").
//   - Known OS + unknown browser → just the OS (e.g. "Windows").
//   - The dux terminal UI's own fixed label → itself, verbatim. It parses as no
//     OS at all, so without this the one non-browser driver dux has would show
//     up on every watcher's card as the generic "another device".
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
