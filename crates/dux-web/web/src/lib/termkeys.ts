// Pure, dependency-free terminal key-synthesis helpers.
//
// These functions translate logical key intents (a control-modified character,
// an arrow press, a chunk of typed text with sticky modifiers) into the raw
// byte sequences a PTY expects. They are intentionally free of any React, DOM,
// or window access so they can be exhaustively unit-tested in isolation and
// reused from any caller (the mobile accessory bar, the onData transform, etc.).

// The ASCII escape character — the lead byte of every CSI/SS3 sequence and the
// Alt (Meta) prefix.
export const ESC = "\x1b"

// The horizontal-tab byte.
export const TAB = "\x09"

// Punctuation/whitespace that map to a control byte. Letters are handled
// arithmetically (see `ctrlByte`); these are the standard caret-notation
// mappings a terminal recognizes:
//   Ctrl-@      -> 0x00 (NUL)
//   Ctrl-[      -> 0x1B (ESC)
//   Ctrl-\      -> 0x1C (FS)
//   Ctrl-]      -> 0x1D (GS)
//   Ctrl-^      -> 0x1E (RS)
//   Ctrl-_      -> 0x1F (US)
//   Ctrl-Space  -> 0x00 (NUL, same as Ctrl-@)
const CTRL_PUNCTUATION: Record<string, number> = {
  "@": 0x00,
  "[": 0x1b,
  "\\": 0x1c,
  "]": 0x1d,
  "^": 0x1e,
  _: 0x1f,
  " ": 0x00,
}

/**
 * Maps a single character to its control byte, or `null` when the character has
 * no control mapping.
 *
 * - `a`-`z` and `A`-`Z` map to `0x01`-`0x1A` (Ctrl-A .. Ctrl-Z), case-folded.
 * - The standard control punctuation (`@ [ \ ] ^ _` and Space) map per the
 *   table above.
 * - Everything else returns `null`.
 */
export function ctrlByte(ch: string): string | null {
  if (ch.length !== 1) return null
  const lower = ch.toLowerCase()
  if (lower >= "a" && lower <= "z") {
    // 'a' (0x61) -> 0x01, ..., 'z' (0x7a) -> 0x1A.
    return String.fromCharCode(lower.charCodeAt(0) - 0x60)
  }
  if (ch in CTRL_PUNCTUATION) {
    return String.fromCharCode(CTRL_PUNCTUATION[ch])
  }
  return null
}

/**
 * Returns the byte sequence for an arrow key.
 *
 * Terminals encode arrows two ways depending on the active cursor-key mode:
 * - Normal (DECCKM reset): CSI form, `ESC [ A/B/C/D`.
 * - Application (DECCKM set): SS3 form, `ESC O A/B/C/D`.
 *
 * Pass the terminal's current `applicationCursorKeys` mode so full-screen apps
 * (vim, less, TUIs) that enable application cursor keys receive the form they
 * expect.
 */
export function arrowSeq(
  dir: "up" | "down" | "left" | "right",
  applicationCursorKeys: boolean,
): string {
  const final = { up: "A", down: "B", right: "C", left: "D" }[dir]
  return `${ESC}${applicationCursorKeys ? "O" : "["}${final}`
}

/**
 * Applies sticky modifiers to a single typed chunk.
 *
 * - `ctrl`: maps the character via `ctrlByte`, falling back to the raw
 *   character when it has no control mapping.
 * - `alt`: prefixes the (possibly ctrl-transformed) result with ESC, the Meta
 *   convention.
 * - Both combine: alt+ctrl yields `ESC` + the control byte.
 *
 * Multi-character chunks (paste, IME composition) pass through UNTRANSFORMED —
 * sticky modifiers are a single-key concept and applying them to a paste would
 * corrupt it. Callers should still clear their one-shot latches after calling
 * this, regardless of whether a transform occurred.
 */
export function applyModifiers(
  data: string,
  mods: { ctrl: boolean; alt: boolean },
): string {
  if (data.length !== 1) return data
  let out = data
  if (mods.ctrl) {
    out = ctrlByte(data) ?? data
  }
  if (mods.alt) {
    out = ESC + out
  }
  return out
}
