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

/** What the terminal should do with a clipboard key chord. */
export type ClipboardKeyAction = "copy" | "paste" | "passthrough"

/**
 * The minimal slice of a `KeyboardEvent` the clipboard classifier reads. We
 * deliberately omit `key`: xterm decides `Ctrl-V`->`\x16` POSITIONALLY by
 * `keyCode`, so we must match the same physical-key signal it uses. Keying off
 * `key` would silently miss on non-Latin layouts (where the V key types e.g.
 * Cyrillic `м`) and let xterm leak `\x16` to the remote agent — the original
 * remote-clipboard bug. `isMac` is supplied by the caller so this stays pure.
 */
export interface ClipboardKeyEvent {
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
  code: string
  keyCode: number
  isMac: boolean
}

/**
 * Classifies a keydown into a clipboard action for the web terminal.
 *
 * - `copy`        -> the caller copies `term.getSelection()` (Ctrl-Shift-C, Ctrl-Insert).
 * - `paste`       -> the caller lets the browser's native paste event flow (Ctrl-V, Ctrl-Shift-V).
 * - `passthrough` -> xterm handles the key normally (Ctrl-C stays SIGINT, plain
 *                    typing is untouched, mac Cmd/Control fall through to the app/browser).
 *
 * Matching is by physical key (`code`, falling back to `keyCode` when `code` is
 * empty) so it works across keyboard layouts. See `ClipboardKeyEvent`.
 */
export function classifyClipboardKey(ev: ClipboardKeyEvent): ClipboardKeyAction {
  // Cmd combos (mac) are left entirely to the browser's native copy/paste.
  if (ev.metaKey) return "passthrough"

  const matches = (code: string, keyCode: number): boolean =>
    ev.code === code || (ev.code === "" && ev.keyCode === keyCode)
  const isV = matches("KeyV", 86)
  const isC = matches("KeyC", 67)
  const isInsert = matches("Insert", 45)

  // On macOS, Cmd already drives the clipboard, so a lone Control modifier must
  // keep reaching the app (vim visual-block, readline verbatim-insert, SIGINT).
  // The Ctrl-Shift aliases below still apply because they also carry Shift.
  if (ev.isMac && ev.ctrlKey && !ev.shiftKey && !ev.altKey) return "passthrough"

  // Alt as a third level (AltGr / Meta) is never a clipboard chord.
  if (ev.altKey) return "passthrough"

  if (ev.ctrlKey && ev.shiftKey && isC) return "copy"
  if (ev.ctrlKey && ev.shiftKey && isV) return "paste"
  if (ev.ctrlKey && !ev.shiftKey && isV) return "paste"
  // Ctrl-C without Shift stays SIGINT (`\x03`) — explicit for intent.
  if (ev.ctrlKey && !ev.shiftKey && isC) return "passthrough"
  if (ev.ctrlKey && isInsert) return "copy"

  // Shift-Insert (browser/OS default paste, source-dependent) and everything
  // else is left to xterm / the browser.
  return "passthrough"
}
