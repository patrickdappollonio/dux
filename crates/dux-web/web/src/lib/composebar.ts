// Pure send rules for the mobile compose bar's Send action.
//
// The compose bar lets a phone user type into a real textarea (native
// autocorrect, swipe input, local editing) and deliver the finished message to
// the PTY. This module owns what the delivery LOOKS like on the wire; the
// component (`ComposeBar.tsx`) owns the input surface, and `TerminalPane.tsx`
// owns the PTY write and its side effects. Like `termkeys.ts`, it is
// deliberately free of any xterm/React/DOM import so the whole matrix is
// unit-testable in isolation (see `composebar.test.ts`).

import { macroPayloadBytes } from "./macros"

// The carriage-return byte (CR, 0x0D), what a plain Enter sends. Interactive
// CLIs treat it as submit, which is exactly what Send means.
const CR = 0x0d

// The largest payload a compose Send will put on the wire, in bytes. The
// server enforces a 16 MiB per-frame cap (`MAX_WS_MESSAGE_SIZE` in
// `crates/dux-web/src/server.rs`) and an oversized frame ABORTS the whole PTY
// socket, so without a client-side check a giant accidental paste would kill
// the connection instead of failing one send. 2 MiB is far more than any real
// composed message while staying well under the server's limit.
export const MAX_COMPOSE_SEND_BYTES = 2 * 1024 * 1024

/**
 * True when a send totalling `payloadBytes` (the summed byte length of its
 * writes) exceeds [`MAX_COMPOSE_SEND_BYTES`]. Callers measure the built
 * bytes, so multi-byte text (CJK, emoji) counts at its real wire size.
 */
export function composeSendTooLarge(payloadBytes: number): boolean {
  return payloadBytes > MAX_COMPOSE_SEND_BYTES
}

// How long the caller waits between the body write and the submitting CR
// write of a non-empty send (see `composeSendWrites`). MEASURED, not guessed:
// the installed Claude Code 2.1.217 bundle merges stdin chunks into one paste
// through a 50ms debounce (and force-classifies any single key event over 800
// chars as a paste), so an Enter arriving with or within 50ms of the body is
// swallowed into the paste as a newline instead of submitting, intermittently,
// depending on chunk timing and length. 150ms is 3x that window with margin,
// still imperceptible next to the tap that triggered the send. This is also
// how a human behaves: paste, a beat, then Enter, as separate key events.
export const COMPOSE_SUBMIT_DELAY_MS = 150

/**
 * Builds the ordered PTY writes a compose-bar Send performs: the message as a
 * MACRO-style keystroke stream, submitted by an Enter that travels alone.
 *
 * The body is exactly [`macroPayloadBytes`] (the transform the macro
 * quick-picker already uses against these same PTYs): every newline, in any
 * of its `\r\n` / `\n` / `\r` spellings, becomes Alt+Enter (ESC CR), the
 * soft-newline keystroke agent CLIs treat as newline-without-submit. This
 * deliberately does not use bracketed paste: the keystroke stream keeps "line
 * break" and "Enter" distinct on the wire, so nothing here depends on a paste
 * protocol the app may or may not negotiate.
 *
 * Two elements mean the caller writes the body immediately and the bare-CR
 * submit after [`COMPOSE_SUBMIT_DELAY_MS`] (see that constant for the measured
 * paste-debounce reason). One element is a single immediate write: an EMPTY
 * buffer sends just the bare CR, a plain Enter, which is how the user confirms
 * a TUI menu or permission prompt without ever focusing xterm; it is a lone
 * keystroke with no body for a paste heuristic to merge it into, so it is
 * never delayed. Whitespace-only text is real text, not empty.
 */
export function composeSendWrites(text: string): Uint8Array[] {
  const body = macroPayloadBytes(text)
  const submit = new Uint8Array([CR])
  if (body.byteLength === 0) return [submit]
  return [body, submit]
}

/**
 * Splices `text` into the compose draft, returning the new draft and where the
 * caret lands (immediately after the inserted text). This is what a picked
 * macro does while the compose bar is the typing surface: the macro becomes an
 * editable draft the user reviews and Sends, never an immediate wire write.
 *
 * `selectionStart`/`selectionEnd` are the textarea's reported selection, in
 * UTF-16 code units (the same units `String.slice` uses, so no conversion). A
 * missing half (`null`) means the caret state is unavailable and the insert
 * APPENDS to the end; out-of-range values (a DOM value briefly lagging the
 * controlled state) are clamped to the draft's bounds, and a reversed
 * selection is reordered before splicing. Multi-line text is inserted
 * verbatim: the draft keeps real newlines, and only the Send path converts
 * them to newline-without-submit keystrokes on the wire.
 */
export function insertIntoComposeDraft(
  draft: string,
  selectionStart: number | null,
  selectionEnd: number | null,
  text: string,
): { next: string; caret: number } {
  let start: number
  let end: number
  if (selectionStart === null || selectionEnd === null) {
    start = draft.length
    end = draft.length
  } else {
    start = Math.min(Math.max(selectionStart, 0), draft.length)
    end = Math.min(Math.max(selectionEnd, 0), draft.length)
    if (start > end) [start, end] = [end, start]
  }
  const next = draft.slice(0, start) + text + draft.slice(end)
  return { next, caret: start + text.length }
}

/**
 * The three values `ui.compose_bar` can take, mirroring
 * `dux_core::config::ComposeBarMode`.
 */
export type ComposeBarMode = "auto" | "always" | "never"

/** Narrow whatever the bootstrap document carried into a mode we have a case
 * for. An absent field (an older server) and an unrecognized one both read as
 * `"auto"`, matching the server-side warn-and-degrade fallback. */
export function composeBarMode(raw: string | undefined): ComposeBarMode {
  return raw === "always" || raw === "never" || raw === "auto" ? raw : "auto"
}

/**
 * Which typing surface a device-local toggle has been left on, or `null` while
 * nobody has touched it and the pointer capability answers. Persisted in
 * `localStorage` by `lib/typingSurface.ts`; deliberately NOT configuration.
 */
export type TypingSurfaceChoice = "compose" | "direct"

/**
 * What the POINTER says the typing surface should be, and nothing more.
 *
 * A coarse pointer is a finger, and a finger wants a buffer where autocorrect,
 * swipe and IMEs have something to work with. This is a DEFAULT, not a gate:
 * the browser is guessing, and a guess loses to the person using the machine.
 * MEASURED, and this is exactly why it can only ever be a default: an Android
 * tablet with a physical keyboard attached and the same tablet without one
 * report identical interaction media queries, so the two cases that want
 * opposite answers are indistinguishable from here.
 */
export function detectedTypingSurface(
  coarsePointer: boolean
): TypingSurfaceChoice {
  return coarsePointer ? "compose" : "direct"
}

/**
 * THE ONE RESOLVED ANSWER to "where does typing go in this pane".
 *
 * `always`/`never` are configuration, written in the Preferences dialog, and
 * they win outright: a per-device toggle must never quietly defeat what the
 * operator set up. `auto` MEANS "work it out", and there the order is the
 * person first and the browser second: an explicit choice, else the pointer's
 * default.
 *
 * The choice wins in BOTH directions on EVERY device, deliberately. Turning the
 * message box on where the pointer says it is not needed is allowed: a laptop
 * user who wants a buffered box, or who is driving a touchscreen the browser
 * has not noticed, is not wrong about their own machine.
 *
 * The pointer changing (a convertible folding, a mouse being unplugged) moves
 * the DEFAULT only. An explicit choice persists until the user changes it,
 * because a surface that snapped back underneath someone mid-session is
 * precisely what makes a toggle feel broken.
 */
export function resolvedTypingSurface(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): TypingSurfaceChoice {
  switch (mode) {
    case "always":
      return "compose"
    case "never":
      return "direct"
    case "auto":
      return choice ?? detectedTypingSurface(coarsePointer)
  }
}

/**
 * Is the buffered message box the typing surface right now?
 *
 * Pure so the whole matrix is testable without mounting a terminal. The
 * capability argument is "is touch the primary pointer" (`pointer: coarse`,
 * see `hooks/use-coarse-pointer.ts`), NOT a viewport width: the bar is a
 * decision about the INPUT METHOD, and keying it to width meant rotating a
 * tablet changed the typing surface underneath the user mid-session.
 */
export function composeBarShown(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): boolean {
  return resolvedTypingSurface(mode, coarsePointer, choice) === "compose"
}

/**
 * Do the TOUCH TYPING SURFACES belong on this device at all?
 *
 * TWO ORTHOGONAL QUESTIONS, and treating them as one was the bug. WIDTH decides
 * the LAYOUT: how much room is there, so which shell you get. The POINTER
 * decides the DEFAULT TYPING SURFACE: is a finger doing the typing, so does the
 * text need a buffer where autocorrect and swipe have something to work with. A
 * tablet in landscape wants the desktop layout AND the buffered input, so the
 * bars travel with the pointer and render inside the desktop shell too.
 *
 * This gates the pair: the accessory keys (Esc/Tab/Ctrl/Alt and the rest) and
 * the compose bar. A coarse pointer always has them, whatever the surface
 * resolves to: `never` and a stored `direct` both keep the accessory keys,
 * because each is a statement about the compose BOX and a soft keyboard still
 * cannot produce a Ctrl chord. A fine pointer gets them once the message box is
 * up, by the setting or by the user's own choice, so asking for the box on a
 * laptop brings its keys along instead of leaving the press inert.
 */
export function touchSurfacesApply(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): boolean {
  return coarsePointer || composeBarShown(mode, coarsePointer, choice)
}

/**
 * Should the IN-BAR toggle render?
 *
 * It lives in the accessory bar's key row, so it is offered exactly where that
 * row is, and only in `auto` (under always/never the setting has already
 * decided, and a control that changed nothing would be a lie). The quick toggle
 * stays in the key row because that is where a thumb already is, and both it
 * and the menu item write through the SAME `setTypingSurface` helper so the two
 * can never disagree about what a switch means.
 */
export function typingSurfaceToggleOffered(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): boolean {
  return mode === "auto" && touchSurfacesApply(mode, coarsePointer, choice)
}

/**
 * Should the INPUT MENU's typing-surface item render?
 *
 * Under `auto`, always, on every device. The menu is the guaranteed way in and
 * out of the virtual input: the key row can be absent (a fine pointer typing
 * directly) and the compose bar with it, so gating this on either of them is
 * what left a laptop user with no way to ask for the message box at all, beside
 * a keys item that did nothing when pressed.
 *
 * Still nothing under `always`/`never`: the setting has decided, and the
 * per-device choice deliberately cannot defeat it.
 */
export function inputMenuSurfaceSwitchOffered(mode: ComposeBarMode): boolean {
  return mode === "auto"
}

/**
 * The cursor style xterm should paint while it does NOT have focus.
 *
 * Verified against the installed @xterm/xterm 6.0.0: `cursorInactiveStyle`
 * accepts 'outline' | 'block' | 'bar' | 'underline' | 'none', defaults to
 * 'outline', and is not in the read-only option list (only `cols` and `rows`
 * are), so it can be reassigned on a live terminal.
 *
 * The compose bar changes what "unfocused" MEANS. Normally an unfocused
 * terminal is one you are not typing at, and every real emulator hollows the
 * caret to say so. With the compose bar up, xterm is never focused by design
 * (the textarea holds focus for the whole session) while the prompt on screen
 * is the live one, so the outline states something false all the time. That
 * mode gets the solid block; direct typing keeps the convention, because there
 * the unfocused caret means exactly what it says.
 */
export function inactiveCursorStyle(
  composeBarActive: boolean
): "block" | "outline" {
  return composeBarActive ? "block" : "outline"
}
