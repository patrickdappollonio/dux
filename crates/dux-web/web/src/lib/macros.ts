// Pure, unit-tested helpers for the web macro surfaces (the terminal-pane
// popover quick-picker and the macro-editor dialog). The surface gate mirrors
// `dux_core`'s rules for fast client-side feedback. Since Phase 5 the web sends a
// macro by writing its payload straight to the focused PTY socket (no server-side
// `run_macro` command), so the byte transform `macroPayloadBytes` is mirrored
// here from `dux_core::macros::macro_payload_bytes` — see its doc comment.

import type { MacroSurface, MacroView } from "@/lib/types"
import type { SelectedTarget } from "@/lib/store"

// Build the byte payload for a macro send. Newlines are translated to Alt+Enter
// (ESC followed by CR) so a multi-line macro is entered as a single multi-line
// prompt rather than submitting at each newline; `\r\n`, `\n`, and bare `\r` are
// all handled. An EXACT port of `dux_core::macros::macro_payload_bytes` (operating
// on UTF-8 bytes, so multi-byte glyphs pass through untouched) — the web now owns
// this transform because it writes the payload directly to the PTY socket.
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
// feedback.
//
// FAST-FEEDBACK MIRROR ONLY (council decision): the authoritative validation is
// `WireCommand::UpdateMacros` in `crates/dux-core/src/wire.rs` (the
// `wire_to_command` arm), which re-runs these rules server-side on every Save.
// This mirror exists purely for instant UI feedback and is intentionally NOT
// pinned cross-language: it's a behavioral rule, not a static contract like the
// palette id pins, so no test ties the two together. If the mirror drifts, the
// worst case is fail-SAFE — a too-lenient client lets a Save through that the
// server then rejects. A too-strict client would only over-block, never corrupt
// state. So the server stays the single source of truth.
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

// Pure commit reducer for the editor's per-row form submission: appends when
// adding ("new"), otherwise replaces the entry at `index` in place. In-place
// replacement is what makes a rename keep its list position (edit entry 0 →
// still index 0), and the append path preserves declaration order. Lives here
// (not in the dialog component) so it stays unit-testable and the dialog file
// keeps exporting only components. Returns a new array; never mutates `prev`.
export function commitMacro(
  prev: MacroView[],
  index: number | "new",
  macro: MacroView,
): MacroView[] {
  if (index === "new") return [...prev, macro]
  return prev.map((m, i) => (i === index ? macro : m))
}

// Sortable ids for the editor list's drag-and-drop, one per macro, POSITIONAL
// ("macro-0", "macro-1", ...). Deliberately not name-based: a transiently
// invalid draft can hold duplicate names, and dnd-kit requires unique ids. The
// list only changes on drop, so positional ids are stable for the whole drag.
export function macroDragIds(macros: MacroView[]): string[] {
  return macros.map((_, index) => `macro-${index}`)
}

// Apply a drag's end to the macro list: move the macro at `activeId`'s slot to
// `overId`'s slot (the same `moveItem` semantics the sidebar reorders use).
// Returns `prev` unchanged (same reference) for a same-slot drop or an id that
// doesn't name a list slot, so callers can cheaply detect the no-op.
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
