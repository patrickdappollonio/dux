// Whether the client is an Apple platform. Drives the command-palette shortcut
// label (Cmd+K vs Ctrl+K, both wired) and the terminal clipboard policy: on Mac
// the native Cmd shortcuts own copy/paste, so a lone Control modifier passes
// through to the app instead of being hijacked (see `classifyClipboardKey`).
export function isApplePlatform(): boolean {
  const platform =
    // Modern Chromium exposes userAgentData; fall back to navigator.platform.
    (navigator as { userAgentData?: { platform?: string } }).userAgentData
      ?.platform ?? navigator.platform
  return /mac|iphone|ipad|ipod/i.test(platform)
}

export function paletteShortcutLabel(): string {
  return isApplePlatform() ? "\u2318K" : "Ctrl K"
}

// The shortcut as discrete key tokens, so UI can render each as its own element
// with uniform spacing. A single string carries a font-dependent space between
// keys that won't match the surrounding layout's gap (and renders wide in a
// monospace context) \u2014 exactly the spacing/alignment issue this avoids.
export function paletteShortcutKeys(): string[] {
  return isApplePlatform() ? ["\u2318", "K"] : ["Ctrl", "K"]
}
