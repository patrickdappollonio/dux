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

// The line-feed byte (LF, 0x0A) — the byte Ctrl-j produces. dux maps Shift-Enter
// to it so a "soft" newline can be inserted into an agent prompt without a
// dedicated Ctrl-j reflex. Interactive CLIs (Claude, Codex, ...) treat a bare LF
// as a literal newline and a carriage return (CR, 0x0D — what a plain Enter
// sends) as submit, so the two must stay distinct.
//
// Note: a newline embedded in *macro* text uses a different encoding — Alt+Enter
// (ESC + CR), see `macros.ts` `macroPayloadBytes`. That path replays a whole
// prewritten prompt as one wholesale write, where Alt+Enter is the reliable
// "newline, don't submit" signal; this path is a single live keystroke, where
// Ctrl-j/LF is the natural chord. Different contexts, deliberately different bytes.
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
 * - `a`-`z` and `A`-`Z` map to `0x01`-`0x1A` (Ctrl-a .. Ctrl-z), case-folded.
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

// Mouse input is replayed through `lib/termmouse.ts` so xterm selects the
// negotiated protocol and cell coordinates. Do not encode mouse reports here.

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

/**
 * Decides whether a terminal keydown should be rewritten to a "soft" newline.
 *
 * Shift-Enter — with no other modifier held — returns `LF` (0x0A, the Ctrl-j
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

// The bytes a bare F-key sends, matched against the TUI's own PTY encoder
// (`crates/dux-tui/src/key_encode.rs`, the `KeyCode::F(n)` arm), which itself
// encodes the standard xterm forms: F1-F4 are the SS3 sequences ESC O P/Q/R/S,
// F5 and up are the CSI tilde forms with their historical gaps (14 is skipped
// to 15, 16 and 22 do not exist). Keeping the two encoders in agreement means
// a hardware F-key does the same thing whether dux is the terminal or the web
// page in front of one.
const FKEY_SEQ: Record<string, string> = {
  F1: `${ESC}OP`,
  F2: `${ESC}OQ`,
  F3: `${ESC}OR`,
  F4: `${ESC}OS`,
  F5: `${ESC}[15~`,
  F6: `${ESC}[17~`,
  F7: `${ESC}[18~`,
  F8: `${ESC}[19~`,
  F9: `${ESC}[20~`,
  F10: `${ESC}[21~`,
  F11: `${ESC}[23~`,
  F12: `${ESC}[24~`,
}

/**
 * Decides whether a physical keydown inside the COMPOSE TEXTAREA is forwarded
 * to the PTY, returning the bytes to send or `null` to leave the key to the
 * browser.
 *
 * The compose bar buffers typing, so while it is focused a physical keyboard's
 * Escape lands in the textarea (where it means nothing) instead of the PTY
 * (where it interrupts a running agent). A hardware Esc is the physical twin
 * of tapping the accessory bar's Esc key, so it earns the same bytes on the
 * same write path; the F-keys ride along because they are equally meaningless
 * to a textarea and equally standard on the wire (see `FKEY_SEQ` for the
 * source the sequences are matched against).
 *
 * The tier is deliberately just Escape and F1-F12:
 *
 *  - Every key with a textarea meaning (printables, Backspace/Delete, arrows,
 *    Home/End, Tab, Enter) keeps it; the buffer is the point of the bar.
 *  - EVERY modified press stays browser-side. Ctrl-c is copy in a browser and
 *    SIGINT in a PTY, and hijacking copy out of a text field is the worse
 *    trade; an Alt/Ctrl/Meta-modified F-key is a browser or OS chord; a
 *    SHIFTED F-key is a different key on the wire (xterm's modified CSI
 *    forms), so sending the plain bytes would lie. Direct mode is the
 *    full-fidelity escape for anyone who needs the rest.
 *  - A bare modifier press (`Control`, `Alt`, ...) is not in the map, so it
 *    falls out as `null` like any other unlisted key.
 *
 * Matching is by `ev.key`, stated as a choice: unlike the clipboard chords
 * (whose letters move with the layout, see `ClipboardKeyEvent`), `Escape` and
 * `F1`-`F12` are layout-independent key values, so the logical and physical
 * key are the same signal here.
 *
 * The IME guard is absolute, same shape as `softNewline`: while composing,
 * Escape is the CANCEL-COMPOSITION key and must keep that local meaning, so a
 * composing event (or the Safari-style `keyCode` 229) forwards nothing. Only
 * `keydown` matches, so keyup/keypress can never double-send.
 */
export function composeHardwareKeyForwards(e: {
  type: string
  key: string
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
  isComposing: boolean
  keyCode: number
}): string | null {
  if (e.type !== "keydown") return null
  if (e.ctrlKey || e.shiftKey || e.altKey || e.metaKey) return null
  if (e.isComposing || e.keyCode === 229) return null
  if (e.key === "Escape") return ESC
  return FKEY_SEQ[e.key] ?? null
}

/** What the terminal should do with a clipboard key chord. */
export type ClipboardKeyAction = "copy" | "paste" | "passthrough"

/**
 * The minimal slice of a `KeyboardEvent` the clipboard classifier reads. We
 * deliberately omit `key`: xterm decides `Ctrl-v`->`\x16` POSITIONALLY by
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

function matchesPhysicalKey(
  event: ClipboardKeyEvent,
  code: string,
  keyCode: number,
): boolean {
  return event.code === code || (event.code === "" && event.keyCode === keyCode)
}

function clipboardModifierGate(
  event: ClipboardKeyEvent,
): ClipboardKeyAction | null {
  if (event.metaKey) return "passthrough"
  if (
    event.isMac &&
    event.ctrlKey &&
    !event.shiftKey &&
    !event.altKey
  ) {
    return "passthrough"
  }
  if (event.altKey) return "passthrough"
  return null
}

function controlClipboardAction(
  event: ClipboardKeyEvent,
): ClipboardKeyAction {
  if (!event.ctrlKey) return "passthrough"
  if (matchesPhysicalKey(event, "Insert", 45)) return "copy"
  if (matchesPhysicalKey(event, "KeyV", 86)) return "paste"
  if (event.shiftKey && matchesPhysicalKey(event, "KeyC", 67)) return "copy"
  return "passthrough"
}

/**
 * Classifies a keydown into a clipboard action for the web terminal.
 *
 * - `copy`        -> the caller copies `term.getSelection()` (Ctrl-Shift-c, Ctrl-Insert).
 * - `paste`       -> the caller lets the browser's native paste event flow (Ctrl-v, Ctrl-Shift-v).
 * - `passthrough` -> xterm handles the key normally (Ctrl-c stays SIGINT, plain
 *                    typing is untouched, mac Cmd/Control fall through to the app/browser).
 *
 * Matching is by physical key (`code`, falling back to `keyCode` when `code` is
 * empty) so it works across keyboard layouts. See `ClipboardKeyEvent`.
 */
export function classifyClipboardKey(ev: ClipboardKeyEvent): ClipboardKeyAction {
  return clipboardModifierGate(ev) ?? controlClipboardAction(ev)
}

/**
 * Whether this chord asks for a TEXT paste specifically, skipping dux's
 * image-wins handling.
 *
 * The escape hatch exists because "an image on the clipboard wins over the text
 * beside it" is right for a screenshot and wrong for rich content: copying a
 * spreadsheet range puts an `image/png` flavour next to the `text/plain` one,
 * and without a way out the numbers are unreachable. `Ctrl+Shift+v` is the
 * natural key for it, since paste-as-plain-text is what that chord means in a
 * browser, an editor and a chat client alike.
 *
 * Deliberately SEPARATE from `classifyClipboardKey` rather than a fourth action
 * of it, for one reason: `Cmd`-anything is classified `passthrough` before any
 * other rule, on purpose (the mac clipboard is the browser's job), so a mac
 * user's `Cmd+Shift+v` never reaches the paste branch and would have lost the
 * hatch. This predicate is asked independently of the classification, so both
 * platforms get the same chord. It only ARMS a preference; the native paste
 * event still flows exactly as it would have.
 *
 * Matched by physical key for the same reason the classifier is: a `key`-based
 * match misses on a non-Latin layout.
 */
export function forcesTextPaste(ev: ClipboardKeyEvent): boolean {
  if (ev.altKey) return false
  if (!ev.shiftKey) return false
  if (!(ev.ctrlKey || ev.metaKey)) return false
  return ev.code === "KeyV" || (ev.code === "" && ev.keyCode === 86)
}

/** What a copy-on-select `mouseup` should do. */
export type CopyOnSelectAction = "copy" | "hint" | "ignore"

/**
 * The runtime context a copy-on-select `mouseup` is judged against. Kept as a
 * plain struct so the branching is pure and unit-testable without mounting xterm.
 */
export interface CopyOnSelectContext {
  /** The `ui.copy_on_select` preference (default on). */
  copyOnSelect: boolean
  /** `term.getSelection()` at mouseup. Empty when no local selection was made. */
  selection: string
  /** Whether the pointer actually moved far enough to count as a drag (not a click). */
  dragged: boolean
  /**
   * `term.modes.mouseTrackingMode`. Anything other than `"none"` means the app in
   * the PTY has grabbed the mouse, so xterm forwarded the drag to the *host*
   * instead of selecting locally — the highlighted text was copied on the host,
   * never on the visitor's machine.
   */
  mouseTrackingMode: string
  /** Whether the mouse-capture hint has already been shown this session. */
  hintShown: boolean
  /**
   * Which gesture produced this selection. Required rather than defaulted, so
   * a new call site has to say which kind it is instead of silently inheriting
   * the mouse's misclick guard (see the length floor below).
   */
  gesture: CopySelectGesture
}

/**
 * The two gestures that can copy on select.
 *
 * `mouse-drag` is a press, a move and a release, any part of which can be an
 * accident. `long-press` is a finger held still for 400ms, which is a
 * deliberate act by construction.
 */
export type CopySelectGesture = "mouse-drag" | "long-press"

/**
 * Decides what a copy-on-select `mouseup` does.
 *
 * - `copy`   -> a real local selection exists; copy it to the visitor's clipboard.
 * - `hint`   -> the user dragged but the app captured the mouse, so nothing was
 *               selected locally (the text went to the host). Surface the
 *               force-selection-modifier hint, once per session.
 * - `ignore` -> preference off, a plain click, or nothing worth acting on.
 *
 * The `selection.length >= 2` floor is the drag-misclick guard: a stray
 * one-char mouse selection never clobbers the clipboard. It applies to a MOUSE
 * DRAG only. A long press cannot be stray (the finger was held still for
 * 400ms), and single-token targets are ordinary in a terminal, so refusing them
 * left the character highlighted and the clipboard untouched with nothing said.
 * Blank is still blank on both paths.
 */
export function copyOnSelectAction(ctx: CopyOnSelectContext): CopyOnSelectAction {
  if (!ctx.copyOnSelect) return "ignore"
  const floor = ctx.gesture === "long-press" ? 1 : 2
  if (ctx.selection.trim().length > 0 && ctx.selection.length >= floor) return "copy"
  if (ctx.dragged && ctx.mouseTrackingMode !== "none" && !ctx.hintShown) return "hint"
  return "ignore"
}

/** What an OSC 8 hyperlink activation should do. */
export type LinkActivateAction = "open" | "ignore"

/**
 * The slice of the `MouseEvent` xterm hands `linkHandler.activate` that decides
 * whether the gesture was a click on the link at all. Both fields matter because
 * xterm's Linkifier reads NEITHER: it activates the link on every `mouseup` that
 * lands on it, whatever the button and however many clicks deep the gesture is.
 */
export interface LinkActivateEvent {
  /** `MouseEvent.button`: 0 primary, 1 middle, 2 secondary. */
  button: number
  /**
   * `MouseEvent.detail`: the running click count of the current multi-click
   * gesture (1 for a plain click, 2 for the second click of a double-click, ...).
   * Synthetic and assistive-technology events may report 0.
   */
  detail: number
  /** `MouseEvent.ctrlKey`: the force-forward hatch chord off an Apple platform. */
  ctrlKey: boolean
  /** `MouseEvent.metaKey`: the force-forward hatch chord on an Apple platform. */
  metaKey: boolean
  /** `MouseEvent.shiftKey`: xterm's force-local-selection modifier off a Mac. */
  shiftKey: boolean
  /** `MouseEvent.altKey`: xterm's force-local-selection modifier on a Mac. */
  altKey: boolean
}

/** The runtime context an activation is judged against. */
export interface LinkActivateContext {
  /** The `capabilities.hyperlinks` preference (default on). */
  hyperlinks: boolean
  /** The URI xterm resolved from the OSC 8 sequence. */
  uri: string
  /** The app in the PTY has mouse reporting on (`mouseTrackingMode !== "none"`). */
  mouseTracking: boolean
  /** Apple platform, which moves the hatch chord from Ctrl to Cmd. */
  isMac: boolean
}

/**
 * Whether the force-forward hatch chord is held.
 *
 * Cmd on macOS, Ctrl everywhere else. It is deliberately NOT the
 * force-local-selection modifier (Shift, or Option on a Mac): xterm's own
 * `mousedown` returns before it encodes anything while that modifier is held,
 * so a click passed through under it forwards zero bytes, and it is already the
 * documented selection hatch, which matters most on a URL, the text people
 * select the most. Cmd and Ctrl both survive xterm's mouse path (meta is not
 * encoded into the SGR modifier bits at all; CAVEAT: Ctrl travels to the app as
 * the +16 modifier bit, so a Linux visitor's hatch click arrives as a
 * ctrl-click rather than a plain one).
 */
export function linkHatchHeld(ev: LinkActivateEvent, isMac: boolean): boolean {
  return isMac ? ev.metaKey : ev.ctrlKey
}

/**
 * Whether the force-LOCAL-SELECTION modifier is held.
 *
 * This mirrors xterm's own `SelectionService.shouldForceSelection`, MEASURED in
 * the installed bundle: `isMac ? altKey && macOptionClickForcesSelection :
 * shiftKey`, and the pane sets `macOptionClickForcesSelection`. Reading the
 * platform the same way matters in both directions. Treating the other
 * platform's modifier as force-selection would leave a press xterm WOULD have
 * forwarded unsuppressed, which is the server-side double open coming back;
 * ignoring this modifier altogether makes a link the one place in the terminal
 * where the documented "select and copy to your own device" gesture does not
 * work, because dux swallows the press and opens a tab out of it instead.
 *
 * Under this modifier xterm's `mousedown` starts a local selection and returns
 * before it sends anything, so passing the press through forwards zero bytes:
 * the app never sees the gesture either way.
 */
export function forceSelectionHeld(ev: LinkActivateEvent, isMac: boolean): boolean {
  return isMac ? ev.altKey : ev.shiftKey
}

/** Only these two schemes are ever handed to the browser. */
const OPENABLE_SCHEME = /^https?:\/\//i

/**
 * Decides whether a hyperlink activation opens a tab.
 *
 * xterm's Linkifier fires `activate` from its `mouseup` listener with no button
 * or click-count check, which produced two user-visible bugs:
 *
 *  - DOUBLE-CLICK OPENED TWICE. Double-clicking is how you select a word in a
 *    terminal, and a triple-click selects the line; each of those extra mouseups
 *    activated the link again, so one ordinary select gesture over a URL spawned
 *    two or three tabs. Only the FIRST click of a gesture is a click, so `detail`
 *    above 1 is the tail of a selection and never an open.
 *  - NON-PRIMARY BUTTONS OPENED IT. In dux a right-click over the terminal is the
 *    PASTE gesture, so right-clicking on a link pasted AND opened a tab; a
 *    middle-click (the X11 primary-selection paste) did the same. Neither button
 *    means "follow this link".
 *
 * The scheme gate is defence in depth: xterm already filters to http(s) unless
 * `allowNonHttpProtocols` is set (we never set it), but the decision to hand an
 * agent-emitted string to `window.open` should be legible in one place.
 *
 * The HATCH rule is the third: while the app in the PTY is tracking the mouse,
 * the hatch chord means "this click belongs to the app", and dux opening a tab
 * as well would be the very double-open the suppression exists to end. With
 * tracking OFF the chord keeps its browser meaning and still opens, because
 * there is no app to hand the click to.
 */
export function linkActivateAction(
  ev: LinkActivateEvent,
  ctx: LinkActivateContext,
): LinkActivateAction {
  if (!ctx.hyperlinks) return "ignore"
  if (ev.button !== 0) return "ignore"
  if (ev.detail > 1) return "ignore"
  if (ctx.mouseTracking && linkHatchHeld(ev, ctx.isMac)) return "ignore"
  // ...and the same for the force-local-selection modifier, for the mirror
  // reason: that gesture is the visitor selecting text, dux does not swallow
  // the press, so xterm's Linkifier still activates the link on the mouseup at
  // the end of the drag. Opening a tab out of a selection is a bug. Only while
  // the app is TRACKING, deliberately: with tracking off xterm's selection is
  // enabled anyway and the modifier keeps its ordinary browser meaning.
  if (ctx.mouseTracking && forceSelectionHeld(ev, ctx.isMac)) return "ignore"
  if (!OPENABLE_SCHEME.test(ctx.uri)) return "ignore"
  return "open"
}

/** The runtime facts a PRESS over the terminal is judged against. */
export interface LinkPressContext {
  /**
   * The URI of the OSC 8 link under the press point, or null for anything else.
   * Resolved synchronously at press time by priming xterm's own Linkifier (see
   * `primeLinkHover`), never by geometry of dux's own.
   */
  hoveredUri: string | null
  /** The app in the PTY has mouse reporting on (`mouseTrackingMode !== "none"`). */
  mouseTracking: boolean
  /** The `capabilities.hyperlinks` preference (default on). */
  hyperlinks: boolean
  /** Apple platform, which moves the hatch chord from Ctrl to Cmd. */
  isMac: boolean
}

/** What the pane does with a press over the terminal. */
export interface LinkPressDecision {
  /**
   * Withhold this press (and its release) from xterm entirely, so no mouse
   * report reaches the app.
   */
  suppress: boolean
  /**
   * This press is eligible to open the link when it is released. Ask
   * `linkReleaseOpens` at release time for the final answer.
   */
  open: boolean
}

/**
 * Decides, AT PRESS TIME, whether a click over the terminal belongs to dux.
 *
 * dux diverges from iTerm2/Ghostty/kitty here, deliberately: a real terminal
 * gives a plain click to the app while it is tracking the mouse and reserves
 * links for a modifier. dux is remote-first, so the app's own "open this URL"
 * runs on the SERVER's machine, where the person who clicked cannot see it. dux
 * is therefore the sole opener, and the click that dispatched a link never
 * reaches the app at all.
 *
 * The decision is at PRESS time because xterm emits the press report from
 * `mousedown`. Deciding at release would already have leaked a lone press, and
 * press-activated TUI controls (buttons, menus) act on exactly that. Keying on
 * the link dispatch is also what keeps those controls intact: a button is not
 * an OSC 8 cell, so its press is never in the suppression set.
 *
 * The two outputs are genuinely different questions, and collapsing them
 * reintroduces the bug from a different direction:
 *
 *  - the second press of a DOUBLE-CLICK (the select-a-word gesture) must still
 *    be swallowed, or a clean click reaches the app and the server-side open is
 *    back, once per extra click;
 *  - a press here and a release somewhere else is a drag, and must open
 *    nothing.
 */
export function linkPressAction(
  ev: LinkActivateEvent,
  ctx: LinkPressContext,
): LinkPressDecision {
  const nothing: LinkPressDecision = { suppress: false, open: false }
  // Tracking off: there was never a report to suppress, and swallowing would
  // cost xterm's focus grab, its selection clear and the copy-on-select
  // listeners, and make a drag-select that starts on a link impossible. Today's
  // Linkifier path stays byte-identical.
  if (!ctx.mouseTracking) return nothing
  if (ctx.hoveredUri === null) return nothing
  // Non-primary buttons keep every contextmenu and paste path untouched.
  if (ev.button !== 0) return nothing
  // The hatch: the visitor asked for the app to have this click, so forward it
  // AND (in `linkActivateAction`) refuse dux's own open.
  if (linkHatchHeld(ev, ctx.isMac)) return nothing
  // The force-LOCAL-SELECTION modifier: the visitor is selecting text, not
  // following a link. Passing the press through is what lets xterm start the
  // selection, and it forwards nothing either way (see `forceSelectionHeld`),
  // so this costs the app nothing and keeps the documented escape hatch working
  // on the text people most want to copy.
  if (forceSelectionHeld(ev, ctx.isMac)) return nothing
  // Swallowed but not opened is a real answer: forwarding a press dux will not
  // act on would hand the app a press with no release. It happens for the tail
  // of a multi-click, and for a link dux would refuse to open anyway (the
  // preference toggled off under a link already on screen, or a scheme the
  // browser should not be handed).
  const open =
    linkActivateAction(ev, {
      hyperlinks: ctx.hyperlinks,
      uri: ctx.hoveredUri,
      mouseTracking: ctx.mouseTracking,
      isMac: ctx.isMac,
    }) === "open"
  return { suppress: true, open }
}

/**
 * Decides whether a swallowed press opens its link on release.
 *
 * A press and release on the same spot is a click. A gesture that travelled is
 * only a click if it stayed on the link it started on, which is what makes a
 * press-on-a-link, release-on-a-word drag open nothing.
 */
export function linkReleaseOpens(ctx: {
  /** `LinkPressDecision.open` from the press. */
  open: boolean
  /** The pointer moved less than the drag threshold between press and release. */
  withinDragThreshold: boolean
  /** The link under the RELEASE point, re-resolved the same way as the press. */
  releaseUri: string | null
  /** The link the press landed on. */
  pressedUri: string
}): boolean {
  if (!ctx.open) return false
  if (ctx.withinDragThreshold) return true
  return ctx.releaseUri === ctx.pressedUri
}
