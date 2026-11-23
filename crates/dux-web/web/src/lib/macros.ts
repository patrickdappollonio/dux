// Pure helpers for the web macro surfaces. The web writes a macro's payload straight
// to the focused PTY socket rather than through a server command, so both the byte
// transform and the surface gate are mirrored from `dux_core::macros`.

import { prose, quotedChip, type Prose } from "@/lib/prose"
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

// The surface options for the editor's "Shown on" choice, in config-comment order
// with the wording mirrored from the canonical `config.toml` `[macros]` comment.
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

// Why the macro editor's list cannot be saved: the sentence, naming the macro
// as a chip, and the index of the macro at fault.
export interface MacroProblem {
  message: Prose
  index: number
}

// A client-side validation error for the macro editor, or null when the set is
// valid: a fast-feedback mirror of rules the server re-runs on every Save, and
// deliberately not pinned to it by a test. Drift fails safe, since a lenient client
// only earns a server refusal and a strict one only over-blocks.
export function validateMacros(macros: MacroView[]): MacroProblem | null {
  const seen = new Set<string>()
  for (const [index, macro] of macros.entries()) {
    const name = macro.name.trim()
    if (name === "") return { message: prose`Every macro needs a name.`, index }
    if (seen.has(name)) {
      return { message: prose`Duplicate macro name: ${quotedChip(name)}.`, index }
    }
    seen.add(name)
    if (macro.text === "") {
      return { message: prose`Macro ${quotedChip(name)} needs some text.`, index }
    }
    if (!isMacroSurface(macro.surface)) {
      return { message: prose`Macro ${quotedChip(name)} has an unknown surface.`, index }
    }
  }
  return null
}

// Narrow an arbitrary string to a known `MacroSurface`.
export function isMacroSurface(value: string): value is MacroSurface {
  return value === "agent" || value === "terminal" || value === "both"
}

// A macro as the Macros dialog edits it: `origin` is the index in the saved
// list of the macro it started as, null for one added in the dialog. Matching
// by that index rather than by name keeps a renamed macro one change.
export interface EditedMacro extends MacroView {
  origin: number | null
}

// An editing session of the Macros dialog, whole: the macros as edited, the
// saved list their origins index (the list the edits were made against) and
// its version, and the index in `macros` of the macro on screen.
export interface MacrosEdit {
  macros: EditedMacro[]
  baseline: MacroView[]
  version: string | null
  selected: number | null
}

// A session with nothing edited yet, on no macro.
export function savedMacrosEdit(saved: MacroView[], version: string | null): MacrosEdit {
  return {
    macros: saved.map((macro, origin) => ({ ...macro, origin })),
    baseline: saved.map((macro) => ({ ...macro })),
    version,
    selected: null,
  }
}

// Single-line preview of a macro's text: newlines collapse to a visible glyph, and
// truncation counts characters rather than bytes so a multi-byte glyph never splits.
export function macroTextPreview(text: string, maxChars = 80): string {
  const oneLine = text.replace(/\r\n|\r|\n/g, " ⏎ ")
  const chars = [...oneLine]
  if (chars.length <= maxChars) return oneLine
  return chars.slice(0, maxChars).join("") + "…"
}
