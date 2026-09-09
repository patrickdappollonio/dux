// Pure terminal key-synthesis helpers: logical key intents (a control-modified
// character, an arrow press, typed text with sticky modifiers) to the byte
// sequences a PTY expects. No React, DOM or window access, so any caller can
// reuse them and a test needs no event.

// The ASCII escape character: the lead byte of every CSI/SS3 sequence and the
// Alt (Meta) prefix.
export const ESC = "\x1b"

// The horizontal-tab byte.
export const TAB = "\x09"

// The line-feed byte (LF, 0x0A): what Ctrl-j produces and what dux maps
// Shift-Enter to. Interactive CLIs read a bare LF as a literal newline and a
// carriage return (CR, 0x0D, a plain Enter) as submit, so the two must stay
// distinct. Newlines inside macro text use Alt+Enter instead, a wholesale write
// rather than a live keystroke; see `macroPayloadBytes` in `macros.ts`.
export const LF = "\x0a"

// The standard caret-notation mappings a terminal recognizes for punctuation
// and space; letters are arithmetic in `ctrlByte`:
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

// Ctrl-<digit>, as the caret-notation aliases of the control punctuation above
// (Ctrl-2 == Ctrl-@ == NUL), which is what xterm and friends emit:
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
 * A single character's control byte, or `null` when it has no control mapping.
 * Letters are case-folded onto `0x01`-`0x1A`; punctuation and digits map per the
 * tables above.
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
 * The byte sequence for an arrow key: the CSI form `ESC [ A/B/C/D` with DECCKM
 * reset, the SS3 form `ESC O A/B/C/D` with it set. Pass the terminal's live
 * `applicationCursorKeys` so full-screen apps get the form they expect.
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
 * Page Up / Page Down as the standard CSI tilde sequences `ESC [ 5 ~` and
 * `ESC [ 6 ~`, which do not vary with cursor-key mode.
 */
export function pageKeySeq(dir: "up" | "down"): string {
  return `${ESC}[${dir === "up" ? "5" : "6"}~`
}

/**
 * Applies sticky modifiers to a single typed chunk: `ctrl` maps through
 * `ctrlByte` (falling back to the raw character), `alt` prefixes ESC, and both
 * combine.
 *
 * Multi-character chunks (paste, IME composition) pass through UNTRANSFORMED,
 * because sticky modifiers are a single-key concept and applying them to a paste
 * would corrupt it. Callers clear their one-shot latches either way.
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
 * `LF` (0x0A, the Ctrl-j byte) for a bare Shift-Enter keydown, `null` for every
 * other event, which the caller leaves to xterm (a plain Enter becomes CR, which
 * the agent treats as submit).
 *
 * ONLY the bare chord: with Ctrl or Alt/Meta also held the user is asking for a
 * different control sequence. Only `keydown` matches, because xterm's custom-key
 * handler also fires for `keyup`/`keypress` and the newline must not be emitted
 * twice. A composing event (or the `keyCode` 229 most browsers report) is left
 * strictly alone: intercepting it would inject a stray LF into the middle of a
 * CJK composition and pre-empt xterm's own composition handling.
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
   * The handler must consume the key: cancel it and tell xterm not to encode its
   * own CR. When false, nothing else here applies and the key is left to xterm.
   */
  handled: boolean
  /** Bytes to write to the PTY, or `null` when nothing should be sent (a read-only viewer). */
  send: string | null
  /** Whether consuming this keystroke should clear the one-shot Ctrl/Alt latch. */
  clearLatch: boolean
}

/**
 * Resolves a keydown into the decisions a terminal key handler needs: the chord
 * match from `softNewline`, plus the ownership gate and the latch-clear rule.
 *
 * `send` carries the LF only for the input owner (a non-owner consumes the key
 * visually but injects nothing), and `clearLatch` is set when the owner had a
 * Ctrl/Alt latch armed, so it cannot leak onto the next keystroke.
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

// The bytes a bare F-key sends: the standard xterm forms, SS3 for F1-F4 and CSI
// tilde above that with their historical gaps (no 14, 16 or 22). Must stay in
// step with the TUI's own encoder, the `KeyCode::F(n)` arm of
// `crates/dux-tui/src/key_encode.rs`, or a hardware F-key means two things.
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
 * The bytes a physical keydown inside the COMPOSE TEXTAREA forwards to the PTY,
 * or `null` to leave the key to the browser.
 *
 * Only bare Escape and F1-F12. Every key with a textarea meaning keeps it, and
 * every modified press stays browser-side: Ctrl-c is copy in a browser and
 * SIGINT in a PTY, and a SHIFTED F-key is a different key on the wire (xterm's
 * modified CSI forms), so sending the plain bytes would lie.
 *
 * Matching is by `ev.key`, which is layout-independent for these keys, unlike
 * the clipboard chords (see `ClipboardKeyEvent`). Only `keydown` matches, so
 * keyup/keypress can never double-send, and nothing forwards while an IME is
 * composing, where Escape is the cancel-composition key.
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
 * The slice of a `KeyboardEvent` the clipboard classifier reads. `key` is
 * deliberately absent: xterm decides `Ctrl-v`->`\x16` POSITIONALLY by `keyCode`,
 * so matching on `key` misses on a non-Latin layout (where the V key types e.g.
 * Cyrillic `м`) and lets xterm leak `\x16` to the remote agent. `isMac` comes
 * from the caller so this stays pure.
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
 * - `passthrough` -> xterm handles the key (Ctrl-c stays SIGINT, plain typing is
 *                    untouched, mac Cmd/Control fall through to the app/browser).
 *
 * Matching is by physical key (`code`, falling back to `keyCode` when `code` is
 * empty) so it works across layouts. See `ClipboardKeyEvent`.
 */
export function classifyClipboardKey(ev: ClipboardKeyEvent): ClipboardKeyAction {
  return clipboardModifierGate(ev) ?? controlClipboardAction(ev)
}

/**
 * Whether this chord asks for a TEXT paste specifically, skipping dux's
 * image-wins handling: copying a spreadsheet range puts an `image/png` flavour
 * beside the `text/plain` one, and without a hatch the numbers are unreachable.
 *
 * Asked independently of `classifyClipboardKey` rather than being a fourth
 * action of it, because that classifier answers `passthrough` to anything with
 * `Cmd` held before any other rule, so a mac user's `Cmd+Shift+v` would lose the
 * hatch. It only ARMS a preference; the native paste event still flows. Matched
 * by physical key for the classifier's reason: a `key` match misses on a
 * non-Latin layout.
 */
export function forcesTextPaste(ev: ClipboardKeyEvent): boolean {
  if (ev.altKey) return false
  if (!ev.shiftKey) return false
  if (!(ev.ctrlKey || ev.metaKey)) return false
  return ev.code === "KeyV" || (ev.code === "" && ev.keyCode === 86)
}

/** What a copy-on-select `mouseup` should do. */
export type CopyOnSelectAction = "copy" | "hint" | "ignore"

/** The runtime context a copy-on-select `mouseup` is judged against. */
export interface CopyOnSelectContext {
  /** The `ui.copy_on_select` preference (default on). */
  copyOnSelect: boolean
  /** `term.getSelection()` at mouseup. Empty when no local selection was made. */
  selection: string
  /** Whether the pointer actually moved far enough to count as a drag (not a click). */
  dragged: boolean
  /**
   * `term.modes.mouseTrackingMode`. Anything but `"none"` means the app grabbed
   * the mouse, so xterm forwarded the drag instead of selecting locally.
   */
  mouseTrackingMode: string
  /** Whether the mouse-capture hint has already been shown this session. */
  hintShown: boolean
  /**
   * Which gesture produced this selection. Required rather than defaulted, so a
   * new call site cannot silently inherit the mouse's misclick guard.
   */
  gesture: CopySelectGesture
}

/**
 * `mouse-drag` is a press, a move and a release, any part of which can be an
 * accident; `long-press` is a finger held still, deliberate by construction.
 */
export type CopySelectGesture = "mouse-drag" | "long-press"

/**
 * Decides what a copy-on-select `mouseup` does.
 *
 * - `copy`   -> a real local selection exists; copy it to the visitor's clipboard.
 * - `hint`   -> the user dragged but the app captured the mouse, so nothing was
 *               selected locally. Surface the force-selection-modifier hint once.
 * - `ignore` -> preference off, a plain click, or nothing worth acting on.
 *
 * The two-character floor is the drag-misclick guard, so a stray one-char mouse
 * selection never clobbers the clipboard. It applies to a MOUSE DRAG only: a
 * long press cannot be stray, and single-token targets are ordinary in a
 * terminal. Blank is still blank on both paths.
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
 * whether the gesture was a click on the link at all. xterm's Linkifier reads
 * neither button nor click count: it activates on every `mouseup` on the link.
 */
export interface LinkActivateEvent {
  /** `MouseEvent.button`: 0 primary, 1 middle, 2 secondary. */
  button: number
  /**
   * `MouseEvent.detail`: the running click count of the current multi-click
   * gesture. Synthetic and assistive-technology events may report 0.
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
 * Whether the force-forward hatch chord is held: Cmd on macOS, Ctrl everywhere
 * else. Deliberately NOT the force-local-selection modifier, which forwards zero
 * bytes anyway and is already the documented selection hatch.
 *
 * Caveat: Ctrl travels to the app as the +16 SGR modifier bit, so a hatch click
 * off a Mac arrives as a ctrl-click rather than a plain one. Meta is not encoded
 * into those bits at all.
 */
export function linkHatchHeld(ev: LinkActivateEvent, isMac: boolean): boolean {
  return isMac ? ev.metaKey : ev.ctrlKey
}

/**
 * Whether the force-LOCAL-SELECTION modifier is held.
 *
 * Mirrors xterm's `SelectionService.shouldForceSelection` (`isMac ? altKey &&
 * macOptionClickForcesSelection : shiftKey`, and the pane sets that option).
 * Reading the platform the same way matters both ways: the other platform's
 * modifier would leave a press xterm WOULD have forwarded unsuppressed, and
 * ignoring this one makes a link the one place the documented select-and-copy
 * gesture fails, since dux would swallow the press and open a tab instead.
 *
 * Under it xterm's `mousedown` starts a local selection and returns before
 * sending anything, so passing the press through forwards zero bytes.
 */
export function forceSelectionHeld(ev: LinkActivateEvent, isMac: boolean): boolean {
  return isMac ? ev.altKey : ev.shiftKey
}

/** Only these two schemes are ever handed to the browser. */
const OPENABLE_SCHEME = /^https?:\/\//i

/**
 * Decides whether a hyperlink activation opens a tab.
 *
 * xterm's Linkifier fires `activate` from `mouseup` with no button or
 * click-count check, so both gates live here: only the FIRST click of a gesture
 * is a click (`detail` above 1 is the tail of a double- or triple-click select,
 * which is how a terminal selects a word or a line), and only the primary button
 * follows a link (in dux a right-click is the paste gesture and a middle-click
 * the X11 primary-selection paste).
 *
 * The scheme gate is defence in depth: xterm already filters to http(s) unless
 * `allowNonHttpProtocols` is set, which dux never sets, but handing an
 * agent-emitted string to `window.open` should be legible in one place.
 *
 * While the app in the PTY is tracking the mouse, the hatch chord means the
 * click belongs to the app, so dux refuses its own open; with tracking off there
 * is no app to hand the click to and the chord keeps its browser meaning.
 */
export function linkActivateAction(
  ev: LinkActivateEvent,
  ctx: LinkActivateContext,
): LinkActivateAction {
  if (!ctx.hyperlinks) return "ignore"
  if (ev.button !== 0) return "ignore"
  if (ev.detail > 1) return "ignore"
  if (ctx.mouseTracking && linkHatchHeld(ev, ctx.isMac)) return "ignore"
  // Mirror for the force-local-selection modifier: dux passes that press through
  // so xterm can select, and the Linkifier still activates on the mouseup ending
  // the drag, but a selection must not open a tab. Only while TRACKING; with
  // tracking off the modifier keeps its ordinary browser meaning.
  if (ctx.mouseTracking && forceSelectionHeld(ev, ctx.isMac)) return "ignore"
  if (!OPENABLE_SCHEME.test(ctx.uri)) return "ignore"
  return "open"
}

/** The runtime facts a PRESS over the terminal is judged against. */
export interface LinkPressContext {
  /**
   * The URI of the OSC 8 link under the press point, or null for anything else.
   * Resolved by priming xterm's own Linkifier (`primeLinkHover`), never by
   * geometry of dux's own.
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
  /** Withhold the press and its release from xterm, so no report reaches the app. */
  suppress: boolean
  /**
   * The press is eligible to open the link on release; `linkReleaseOpens` gives
   * the final answer.
   */
  open: boolean
}

/**
 * Decides, AT PRESS TIME, whether a click over the terminal belongs to dux.
 *
 * dux is remote-first, so an app's own "open this URL" runs on the SERVER's
 * machine where the person who clicked cannot see it. dux is therefore the sole
 * opener and the click that dispatched a link never reaches the app, deliberately
 * unlike iTerm2/Ghostty/kitty, which give a tracked click to the app and reserve
 * links for a modifier.
 *
 * At PRESS time because xterm emits the press report from `mousedown`, and
 * press-activated TUI controls act on a lone press. Keying on the link dispatch
 * keeps those controls intact: a button is not an OSC 8 cell, so its press is
 * never in the suppression set.
 *
 * `suppress` and `open` are separate answers: the second press of a double-click
 * must still be swallowed or a clean click reaches the app, and a press here
 * with a release elsewhere is a drag that must open nothing.
 */
export function linkPressAction(
  ev: LinkActivateEvent,
  ctx: LinkPressContext,
): LinkPressDecision {
  const nothing: LinkPressDecision = { suppress: false, open: false }
  // Tracking off: there is no report to suppress, and swallowing would cost
  // xterm's focus grab, its selection clear and the copy-on-select listeners,
  // and make a drag-select that starts on a link impossible.
  if (!ctx.mouseTracking) return nothing
  if (ctx.hoveredUri === null) return nothing
  // Non-primary buttons keep every contextmenu and paste path untouched.
  if (ev.button !== 0) return nothing
  // The hatch: the visitor asked for the app to have this click, so forward it
  // AND (in `linkActivateAction`) refuse dux's own open.
  if (linkHatchHeld(ev, ctx.isMac)) return nothing
  // The force-LOCAL-SELECTION modifier: the visitor is selecting text, not
  // following a link. Passing the press through is what lets xterm start the
  // selection, and it forwards nothing either way (see `forceSelectionHeld`).
  if (forceSelectionHeld(ev, ctx.isMac)) return nothing
  // Swallowed but not opened is a real answer: forwarding a press dux will not
  // act on hands the app a press with no release. It happens for the tail of a
  // multi-click and for a link dux would refuse to open anyway.
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
 * Whether a swallowed press opens its link on release. A gesture that travelled
 * counts only if it stayed on the link it started on, so a press-on-a-link,
 * release-on-a-word drag opens nothing.
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
