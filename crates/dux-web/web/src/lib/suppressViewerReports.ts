import type { Terminal } from "@xterm/xterm"

// dux-core's `alacritty_terminal` is the AUTHORITATIVE emulator on each PTY: it
// parses the child's output, answers the child's device/status/color queries,
// and writes those replies straight back to the PTY master. The browser xterm.js
// is only a VIEWER of that same PTY stream, but by default it is a full emulator
// too, so it ALSO answers those queries. xterm.js routes its answers through
// `onData`, the very same path as typed keystrokes (see TerminalPane's
// `term.onData`), so each answer is written back into the shared PTY a SECOND
// time.
//
// dux-core's reply is consumed by whatever program issued the query. xterm.js's
// duplicate reply arrives a beat later. If the shell is sitting idle at its
// prompt by then (any SIGWINCH can make the prompt re-probe the terminal; the
// reconnect's first-frame jiggle-resize is one common trigger), the duplicate is
// "typed" at the prompt as literal garbage like:
//
//   ]10;rgb:ffff/ffff/ffff]11;rgb:0101/0101/0000[?1;2c
//
// OSC 10 (default foreground), OSC 11 (default background, which here echoes the
// app `--background` we hand xterm as its theme), and the Primary DA `[?1;2c`
// (xterm.js's hardcoded "VT100 with Advanced Video Option" answer).
//
// The fix: make the viewer stop auto-answering EVERY query xterm would reply to,
// so the PTY only ever sees one set of replies (dux-core's) plus genuine user
// input. Custom parser handlers are tried before xterm's built-in ones, and
// returning `true` marks the sequence handled so the built-in responder never
// runs. None of the suppressed sequences change what is rendered (they are pure
// reports), so the viewer's display is unaffected.

// True when an OSC color payload is a QUERY rather than a SET. xterm's own
// handler splits the payload on ";" and replies for any "?" slot, so mirror that
// exactly: suppress precisely when xterm would reply, and let a pure SET (real
// color values, no "?") fall through to the built-in so the viewer still
// recolors. Covers the single ("?"), trailing-semicolon ("?;"), stacked ("?;?")
// and indexed-palette ("5;?") forms alike. Exported for unit testing.
export function isColorQuery(data: string): boolean {
  return data.split(";").includes("?")
}

export function suppressViewerReports(term: Terminal): void {
  // Always report "handled" so xterm's built-in responder is skipped.
  const swallow = () => true

  // Device Attributes, pure capability queries with no display effect:
  //   Primary   CSI c / CSI 0 c
  //   Secondary CSI > c
  // xterm has no Tertiary (`CSI = c`) responder, so none is registered.
  term.parser.registerCsiHandler({ final: "c" }, swallow)
  term.parser.registerCsiHandler({ prefix: ">", final: "c" }, swallow)

  // Device Status Report. xterm only replies to CSI 5 n (status) and CSI 6 n
  // (cursor position), so swallow just those and let any other `CSI Ps n` fall
  // through. The DEC private form (CSI ? Ps n, e.g. ? 6 n) is all reports.
  term.parser.registerCsiHandler(
    { final: "n" },
    (params) => params[0] === 5 || params[0] === 6,
  )
  term.parser.registerCsiHandler({ prefix: "?", final: "n" }, swallow)

  // DECRQM mode reports: xterm answers CSI Ps $ p and CSI ? Ps $ p with a
  // `...$y` status reply (e.g. the bracketed-paste probe CSI ? 2004 $ p that
  // shells and editors commonly send). Pure reports.
  term.parser.registerCsiHandler({ intermediates: "$", final: "p" }, swallow)
  term.parser.registerCsiHandler(
    { prefix: "?", intermediates: "$", final: "p" },
    swallow,
  )

  // DECRQSS status-string reports: xterm answers DCS $ q ... ST. Pure reports.
  term.parser.registerDcsHandler({ intermediates: "$", final: "q" }, swallow)

  // Color queries: OSC 4 (indexed palette) and OSC 10/11/12 (default
  // foreground/background/cursor). Suppress only the QUERY form so a SET (a real
  // color value) falls through to xterm's built-in and the viewer still recolors.
  term.parser.registerOscHandler(4, isColorQuery)
  term.parser.registerOscHandler(10, isColorQuery)
  term.parser.registerOscHandler(11, isColorQuery)
  term.parser.registerOscHandler(12, isColorQuery)
}
