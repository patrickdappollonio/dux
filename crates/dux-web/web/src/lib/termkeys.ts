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

// The line-feed byte (LF, 0x0A) — the byte Ctrl-J produces. dux maps Shift-Enter
// to it so a "soft" newline can be inserted into an agent prompt without a
// dedicated Ctrl-J reflex. Interactive CLIs (Claude, Codex, ...) treat a bare LF
// as a literal newline and a carriage return (CR, 0x0D — what a plain Enter
// sends) as submit, so the two must stay distinct.
//
// Note: a newline embedded in *macro* text uses a different encoding — Alt+Enter
// (ESC + CR), see `macros.ts` `macroPayloadBytes`. That path replays a whole
// prewritten prompt as one wholesale write, where Alt+Enter is the reliable
// "newline, don't submit" signal; this path is a single live keystroke, where
// Ctrl-J/LF is the natural chord. Different contexts, deliberately different bytes.
export const LF = "\x0a"

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

/**
 * Decides whether a terminal keydown should be rewritten to a "soft" newline.
 *
 * Shift-Enter — with no other modifier held — returns `LF` (0x0A, the Ctrl-J
 * byte). Every other event returns `null`, signalling the caller to let xterm
 * encode the key normally (a plain Enter becomes CR, which the agent treats as
 * submit).
 *
 * We deliberately match ONLY the bare Shift-Enter chord: if Ctrl, Alt/Meta is
 * also held the user is asking for a different control sequence, so we leave
 * those to xterm rather than swallowing them. Only `keydown` is matched —
 * xterm's custom-key handler also fires for `keyup`/`keypress`, which must pass
 * through untouched so we never emit the newline twice.
 *
 * An in-flight IME composition is left strictly alone: while composing CJK text
 * the confirming/return keystroke arrives with `isComposing` true (and, on most
 * browsers, `keyCode` 229). If we intercepted it we would inject a stray LF into
 * the middle of the composition and pre-empt xterm's own composition handling,
 * corrupting the text — the exact failure the app's IME accessibility guarantees
 * exist to prevent. So we bail and let the keystroke finalize composition
 * normally.
 *
 * Pure and DOM-free by design: it reads only the plain fields of a
 * `KeyboardEvent`, so it is unit-testable without a real event (see
 * `termkeys.test.ts`).
 */
export function softNewline(e: {
  type: string
  key: string
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
  isComposing: boolean
  keyCode: number
}): string | null {
  if (
    e.type === "keydown" &&
    e.key === "Enter" &&
    e.shiftKey &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.metaKey &&
    !e.isComposing &&
    e.keyCode !== 229
  ) {
    return LF
  }
  return null
}

/** What a terminal key handler should do with a keystroke (see `softNewlineAction`). */
export interface SoftNewlineAction {
  /**
   * The key is a soft-newline chord that the handler must consume: cancel it
   * (`preventDefault`/`stopPropagation`) and tell xterm not to encode its own CR.
   * When false, nothing else in this action applies and the key is left to xterm.
   */
  handled: boolean
  /** Bytes to write to the PTY, or `null` when nothing should be sent (a read-only viewer). */
  send: string | null
  /** Whether consuming this keystroke should clear the one-shot Ctrl/Alt latch. */
  clearLatch: boolean
}

/**
 * Resolves a keydown into the full set of decisions a terminal key handler needs,
 * combining the pure chord match (`softNewline`) with runtime context. Keeping the
 * branching here — rather than inside the component's event closure — makes the
 * ownership gate and the latch-clear rule unit-testable without mounting xterm.
 *
 * - Not a soft-newline chord → `{ handled: false, ... }`: the caller lets xterm handle the key.
 * - A soft-newline chord → `handled: true`; `send` carries the LF only for the input
 *   owner (a non-owner consumes the key visually but injects nothing); `clearLatch`
 *   is set when the owner had an armed Ctrl/Alt latch, so it can't leak onto the
 *   next keystroke.
 */
export function softNewlineAction(
  e: {
    type: string
    key: string
    ctrlKey: boolean
    shiftKey: boolean
    altKey: boolean
    metaKey: boolean
    isComposing: boolean
    keyCode: number
  },
  ctx: { isOwner: boolean; ctrlLatched: boolean; altLatched: boolean },
): SoftNewlineAction {
  const nl = softNewline(e)
  if (nl === null) return { handled: false, send: null, clearLatch: false }
  return {
    handled: true,
    send: ctx.isOwner ? nl : null,
    clearLatch: ctx.isOwner && (ctx.ctrlLatched || ctx.altLatched),
  }
}
