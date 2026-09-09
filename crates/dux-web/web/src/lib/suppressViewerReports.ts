import type { Terminal } from "@xterm/xterm"

// dux-core's `alacritty_terminal` is the AUTHORITATIVE emulator on each PTY: it
// answers the child's device/status/color queries and writes the replies back to
// the PTY master. The browser xterm.js is only a VIEWER of that stream, but by
// default it is a full emulator too and answers the same queries, routing them
// through `onData`, the path typed keystrokes take, so a second set of replies
// is written into the shared PTY. Arriving a beat later, it is "typed" at an
// idle prompt as literal garbage like:
//
//   ]10;rgb:ffff/ffff/ffff]11;rgb:0101/0101/0000[?1;2c
//
// So the viewer must not auto-answer anything xterm would reply to, leaving the
// PTY one set of replies plus genuine user input. Custom parser handlers are
// tried before xterm's built-in ones and returning `true` marks the sequence
// handled, so the built-in responder never runs; none of the suppressed
// sequences change what is rendered, so the display is unaffected.

// True when an OSC color payload is a QUERY rather than a SET. xterm's handler
// splits the payload on ";" and replies for any "?" slot, so mirror that
// exactly: suppress precisely when xterm would reply, and let a pure SET fall
// through to the built-in so the viewer still recolors. Covers the single,
// trailing-semicolon, stacked and indexed-palette forms alike.
export function isColorQuery(data: string): boolean {
  return data.split(";").includes("?")
}

export function suppressViewerReports(term: Terminal): void {
  // Always report "handled" so xterm's built-in responder is skipped.
  const swallow = () => true

  // Device Attributes, pure capability queries: Primary (CSI c / CSI 0 c) and
  // Secondary (CSI > c). xterm has no Tertiary (CSI = c) responder to suppress.
  term.parser.registerCsiHandler({ final: "c" }, swallow)
  term.parser.registerCsiHandler({ prefix: ">", final: "c" }, swallow)

  // Device Status Report. xterm replies only to CSI 5 n and CSI 6 n, so any
  // other `CSI Ps n` falls through; the DEC private form is all reports.
  term.parser.registerCsiHandler(
    { final: "n" },
    (params) => params[0] === 5 || params[0] === 6,
  )
  term.parser.registerCsiHandler({ prefix: "?", final: "n" }, swallow)

  // DECRQM mode reports: xterm answers CSI Ps $ p and CSI ? Ps $ p with a `$y`
  // status reply, such as the bracketed-paste probe CSI ? 2004 $ p. Pure reports.
  term.parser.registerCsiHandler({ intermediates: "$", final: "p" }, swallow)
  term.parser.registerCsiHandler(
    { prefix: "?", intermediates: "$", final: "p" },
    swallow,
  )

  // DECRQSS status-string reports: xterm answers DCS $ q ... ST. Pure reports.
  term.parser.registerDcsHandler({ intermediates: "$", final: "q" }, swallow)

  // Color queries: OSC 4 (indexed palette) and OSC 10/11/12 (default
  // foreground/background/cursor). Only the QUERY form, so a SET still recolors.
  term.parser.registerOscHandler(4, isColorQuery)
  term.parser.registerOscHandler(10, isColorQuery)
  term.parser.registerOscHandler(11, isColorQuery)
  term.parser.registerOscHandler(12, isColorQuery)
}

// The focus reports xterm emits when the app turns focus reporting on:
// FocusIn is `CSI I`, FocusOut is `CSI O`.
export const FOCUS_IN_REPORT = "\x1b[I"
export const FOCUS_OUT_REPORT = "\x1b[O"

// True for a bare focus report and nothing else.
//
// The same "the viewer must not volunteer state" rule as the suppressions above,
// but it cannot be a parser handler: the report is not a reply to an incoming
// sequence, it is xterm VOLUNTEERING its focus state when the app sets DECSET
// 1004, pushed through `onData` like a keystroke (DECRST 1004 pushes nothing).
//
// dux replays a mode-restore tail on EVERY (re)open carrying `?1004h` whenever
// the child had focus reporting on (`dux_core::pty::mode_restore_sequence`), so
// a replay applied to a pane whose terminal element is not focused types a
// spurious focus-OUT at the child, which the claude CLI reacts to. Callers bound
// a suppression window around applying a replay chunk and drop the reports
// raised inside it; a real transition outside that window still goes to the PTY.
export function isFocusReport(data: string): boolean {
  return data === FOCUS_IN_REPORT || data === FOCUS_OUT_REPORT
}
