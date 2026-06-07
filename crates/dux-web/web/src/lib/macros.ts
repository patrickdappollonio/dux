// Pure, unit-tested helpers for the web macro surfaces (the terminal-pane
// popover quick-picker and the macro-editor dialog). The byte transform and the
// authoritative surface gate live in `dux_core` and run engine-side when a macro
// is sent via the `run_macro` command — these helpers only drive the CLIENT'S
// presentation and give fast feedback that mirrors the server's rules.

import type { MacroSurface, MacroView } from "@/lib/types"
import type { SelectedTarget } from "@/lib/store"

// Whether a macro of `macroSurface` is available on a target of `targetKind`.
// Mirrors `dux_core::macros::macro_matches_surface` exactly: "both" is available
// everywhere, "agent" only on an agent target, "terminal" only on a terminal
// target. The popover uses this to filter the list to the focused target.
export function macroMatchesSurface(
  macroSurface: MacroSurface,
  targetKind: "agent" | "terminal",
): boolean {
  switch (macroSurface) {
    case "both":
      return true
    case "agent":
      return targetKind === "agent"
    case "terminal":
      return targetKind === "terminal"
  }
}

// The macros (in config order) available on the focused target's surface. The
// popover renders this; an empty result with a non-empty `macros` means "no
// macros for this target kind", while an empty `macros` means "none at all".
export function macrosForTarget(
  macros: MacroView[],
  target: SelectedTarget,
): MacroView[] {
  return macros.filter((m) => macroMatchesSurface(m.surface, target.kind))
}

// The surface options for the editor's Select, in config-comment order with the
// wording mirrored from the canonical `config.toml` `[macros]` comment.
export const MACRO_SURFACE_OPTIONS: {
  value: MacroSurface
  label: string
  description: string
}[] = [
  {
    value: "agent",
    label: "Agent",
    description: "Only shown when the agent pane is focused.",
  },
  {
    value: "terminal",
    label: "Terminal",
    description: "Only shown when the terminal pane is focused.",
  },
  {
    value: "both",
    label: "Both",
    description: "Shown on both surfaces.",
  },
]

// A client-side validation error for the macro editor, or null when the whole
// set is valid. Mirrors the server's wholesale-replace rules (empty/duplicate
// names, empty text, known surface) so the Save button can give immediate
// feedback; the server stays authoritative and re-validates regardless.
export function validateMacros(macros: MacroView[]): string | null {
  const seen = new Set<string>()
  for (const macro of macros) {
    const name = macro.name.trim()
    if (name === "") return "Every macro needs a name."
    if (seen.has(name)) return `Duplicate macro name: "${name}".`
    seen.add(name)
    if (macro.text === "") return `Macro "${name}" needs some text.`
    if (!isMacroSurface(macro.surface)) {
      return `Macro "${name}" has an unknown surface.`
    }
  }
  return null
}

// Narrow an arbitrary string to a known `MacroSurface`.
export function isMacroSurface(value: string): value is MacroSurface {
  return value === "agent" || value === "terminal" || value === "both"
}

// Single-line preview of a macro's text for the editor list: newlines collapse
// to a visible glyph so a multi-line macro stays one row. Truncation is by
// CHARACTER (not byte) so multi-byte glyphs never split — and capped so a long
// macro can't blow out the row. The popover and dialog both render names; only
// the dialog list needs this preview.
export function macroTextPreview(text: string, maxChars = 80): string {
  const oneLine = text.replace(/\r\n|\r|\n/g, " ⏎ ")
  const chars = [...oneLine]
  if (chars.length <= maxChars) return oneLine
  return chars.slice(0, maxChars).join("") + "…"
}
