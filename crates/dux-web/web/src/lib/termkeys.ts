// Pure, dependency-free terminal key-synthesis helpers.
//
// These functions translate logical key intents (a control-modified character,
// an arrow press, a chunk of typed text with sticky modifiers) into the raw
// byte sequences a PTY expects. They are intentionally free of any React, DOM,
// or window access so they can be unit-tested in isolation (see
// `termkeys.test.ts`) and reused from any caller (the mobile accessory bar, the
// onData transform, etc.).

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

// Digits that map to a control byte, mirroring how a real terminal treats
// Ctrl-<digit>. These reuse the caret-notation aliases of the control
// punctuation above (e.g. Ctrl-2 == Ctrl-@ == NUL), which is the behavior
// xterm and friends emit:
//   Ctrl-2 -> 0x00 (NUL, alias of Ctrl-@)
//   Ctrl-3 -> 0x1B (ESC, alias of Ctrl-[)
//   Ctrl-4 -> 0x1C (FS,  alias of Ctrl-\)
//   Ctrl-5 -> 0x1D (GS,  alias of Ctrl-])
//   Ctrl-6 -> 0x1E (RS,  alias of Ctrl-^)
//   Ctrl-7 -> 0x1F (US,  alias of Ctrl-_)
//   Ctrl-8 -> 0x7F (DEL)
// Digits 0, 1, and 9 have no control mapping and return `null`.
const CTRL_DIGIT: Record<string, number> = {
  "2": 0x00,
  "3": 0x1b,
  "4": 0x1c,
  "5": 0x1d,
  "6": 0x1e,
  "7": 0x1f,
  "8": 0x7f,
}

/**
 * Maps a single character to its control byte, or `null` when the character has
 * no control mapping.
 *
 * - `a`-`z` and `A`-`Z` map to `0x01`-`0x1A` (Ctrl-A .. Ctrl-Z), case-folded.
 * - The standard control punctuation (`@ [ \ ] ^ _` and Space) map per the
 *   table above.
 * - Digits `2`-`8` map to their control aliases (see `CTRL_DIGIT`); `0`, `1`,
 *   and `9` have no mapping.
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
  if (ch in CTRL_DIGIT) {
    return String.fromCharCode(CTRL_DIGIT[ch])
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
 * Encodes a mouse-wheel scroll as SGR (1006) press events for forwarding to a
 * full-screen (alt-screen) app that has mouse tracking enabled.
 *
 * A full-screen TUI owns the alternate screen and keeps its OWN scrollback that
 * never reaches xterm's viewport, so xterm's local `scrollLines()` cannot move
 * it. To scroll such an app from a touch drag or the Page buttons we forward the
 * exact wheel events xterm would synthesize for a real mouse wheel: the SGR
 * press form `ESC [ < Cb ; Col ; Row M`, with button code 64 = wheel up (older
 * output) and 65 = wheel down (newer output). `Col`/`Row` are the 1-based cell
 * under the pointer; most full-screen apps ignore the position for wheel events,
 * but we send a real in-bounds cell to be safe.
 *
 * `lines` is signed the same way as xterm's `scrollLines()`: NEGATIVE reveals
 * OLDER output (wheel up), POSITIVE reveals NEWER output (wheel down). Returns
 * `abs(lines)` stacked wheel events (one per line), or `""` for a zero scroll.
 *
 * This emits SGR encoding unconditionally, which every modern full-screen CLI
 * (Claude, Codex, OpenCode, Copilot, vim, less) negotiates via DECSET 1006. The
 * caller is responsible for only invoking this when `mouseTrackingMode` is not
 * `"none"`; a legacy app that requested non-SGR mouse encoding would misread it.
 */
export function sgrWheelSeq(lines: number, col: number, row: number): string {
  const count = Math.abs(Math.trunc(lines))
  if (count === 0) return ""
  const button = lines < 0 ? 64 : 65
  const c = Math.max(1, Math.trunc(col))
  const r = Math.max(1, Math.trunc(row))
  return `${ESC}[<${button};${c};${r}M`.repeat(count)
}

/**
 * Returns the byte sequence for a Page Up / Page Down key, used to page a
 * full-screen app that scrolls by keyboard rather than mouse (no mouse
 * tracking). These are the standard CSI tilde sequences `ESC [ 5 ~` (PgUp) and
 * `ESC [ 6 ~` (PgDn), which do not vary with cursor-key mode.
 */
export function pageKeySeq(dir: "up" | "down"): string {
  return `${ESC}[${dir === "up" ? "5" : "6"}~`
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
