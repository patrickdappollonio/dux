// The label for the command-palette shortcut, matched to the platform: the
// handler accepts BOTH metaKey and ctrlKey (CommandPalette.tsx), so Cmd+K and
// Ctrl+K always work — but the LABEL should name the key the user actually
// has. Apple platforms get the ⌘ glyph; everything else gets Ctrl.
export function paletteShortcutLabel(): string {
  const platform =
    // Modern Chromium exposes userAgentData; fall back to navigator.platform.
    (navigator as { userAgentData?: { platform?: string } }).userAgentData
      ?.platform ?? navigator.platform
  return /mac|iphone|ipad|ipod/i.test(platform) ? "\u2318K" : "Ctrl K"
}
