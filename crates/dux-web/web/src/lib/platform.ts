// Whether the client is an Apple platform, which decides the terminal clipboard
// policy: Cmd owns copy and paste there, so a lone Control passes through to the app
// and Option forces a local xterm selection.
export function isApplePlatform(): boolean {
  const platform =
    // Modern Chromium exposes userAgentData; fall back to navigator.platform.
    (navigator as { userAgentData?: { platform?: string } }).userAgentData
      ?.platform ?? navigator.platform
  return /mac|iphone|ipad|ipod/i.test(platform)
}
