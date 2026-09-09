// Pure helpers for the web macro surfaces. The web writes a macro's payload straight
// to the focused PTY socket rather than through a server command, so both the byte
// transform and the surface gate are mirrored from `dux_core::macros`.

import type { MacroSurface, MacroView } from "@/lib/types"
import type { SelectedTarget } from "@/lib/store"

// Builds the byte payload for a macro send, an exact port of
// `dux_core::macros::macro_payload_bytes` over UTF-8 bytes. Every newline form
// becomes Alt+Enter (ESC then CR), so a multi-line macro is entered as one prompt
// instead of submitting at each line.
export function macroPayloadBytes(text: string): Uint8Array {
  const ALT_ENTER = [0x1b, 0x0d] // ESC, CR
  const bytes = new TextEncoder().encode(text)
  const out: number[] = []
  let i = 0
  while (i < bytes.length) {
    const b = bytes[i]
    if (b === 0x0d && bytes[i + 1] === 0x0a) {
      out.push(...ALT_ENTER)
      i += 2
    } else if (b === 0x0a || b === 0x0d) {
      out.push(...ALT_ENTER)
      i += 1
    } else {
      out.push(b)
      i += 1
    }
  }
  return new Uint8Array(out)
}

// Whether a macro of `macroSurface` is available on a target of `targetKind`, an
// exact mirror of `dux_core::macros::macro_matches_surface`: "both" everywhere,
// "agent" and "terminal" only on their own target kind.
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

// A client-side validation error for the macro editor, or null when the set is
// valid: a fast-feedback mirror of rules the server re-runs on every Save, and
// deliberately not pinned to it by a test. Drift fails safe, since a lenient client
// only earns a server refusal and a strict one only over-blocks.
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

// Commit reducer for the editor's per-row form: appends when adding, otherwise
// replaces the entry at `index` in place, which is what keeps a renamed macro in its
// list position. Returns a new array and never mutates `prev`.
export function commitMacro(
  prev: MacroView[],
  index: number | "new",
  macro: MacroView,
): MacroView[] {
  if (index === "new") return [...prev, macro]
  return prev.map((m, i) => (i === index ? macro : m))
}

// Sortable ids for the editor list's drag and drop, positional rather than
// name-based: a draft may transiently hold duplicate names and the ids must be
// unique. The list changes only on drop, so they are stable for a whole drag.
export function macroDragIds(macros: MacroView[]): string[] {
  return macros.map((_, index) => `macro-${index}`)
}

// Applies a drag's end, moving the macro at `activeId`'s slot to `overId`'s.
// Returns `prev` by reference for a same-slot drop or an id that names no slot, so a
// caller can detect the no-op.
export function reorderMacrosByDrag(
  prev: MacroView[],
  activeId: string,
  overId: string,
): MacroView[] {
  if (activeId === overId) return prev
  const ids = macroDragIds(prev)
  const from = ids.indexOf(activeId)
  const to = ids.indexOf(overId)
  if (from === -1 || to === -1) return prev
  const next = prev.slice()
  const [moved] = next.splice(from, 1)
  next.splice(to, 0, moved)
  return next
}

// Single-line preview of a macro's text: newlines collapse to a visible glyph, and
// truncation counts characters rather than bytes so a multi-byte glyph never splits.
export function macroTextPreview(text: string, maxChars = 80): string {
  const oneLine = text.replace(/\r\n|\r|\n/g, " ⏎ ")
  const chars = [...oneLine]
  if (chars.length <= maxChars) return oneLine
  return chars.slice(0, maxChars).join("") + "…"
}
