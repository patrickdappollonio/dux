// The label for the command-palette shortcut, matched to the platform: the
// handler accepts BOTH metaKey and ctrlKey (CommandPalette.tsx), so Cmd+K and
// Ctrl+K always work — but the LABEL should name the key the user actually
// has. Apple platforms get the ⌘ glyph; everything else gets Ctrl.
function isApplePlatform(): boolean {
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
