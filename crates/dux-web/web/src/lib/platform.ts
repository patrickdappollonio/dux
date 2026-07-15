// Whether the client is an Apple platform. Drives the terminal clipboard policy:
// on Mac the native Cmd shortcuts own copy/paste, so a lone Control modifier
// passes through to the app instead of being hijacked (see
// `classifyClipboardKey`), and Option forces a local xterm selection.
export function isApplePlatform(): boolean {
  const platform =
    // Modern Chromium exposes userAgentData; fall back to navigator.platform.
    (navigator as { userAgentData?: { platform?: string } }).userAgentData
      ?.platform ?? navigator.platform
  return /mac|iphone|ipad|ipod/i.test(platform)
}
