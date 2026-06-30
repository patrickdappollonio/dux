import type { Terminal } from "@xterm/xterm"

// dux-core's `alacritty_terminal` is the AUTHORITATIVE emulator on each PTY: it
// parses the child's output, answers the child's device/status/color queries
// (Primary/Secondary DA, DSR, OSC 10/11/12 color queries), and writes those
// replies straight back to the PTY master. The browser xterm.js is only a
// VIEWER of that same PTY stream, but by default it is a full emulator too, so
// it ALSO answers those queries. xterm.js routes its answers through `onData`,
// the very same path as typed keystrokes (see TerminalPane's `term.onData`), so
// each answer is written back into the shared PTY a SECOND time.
//
// dux-core's reply is consumed by whatever program issued the query. xterm.js's
// duplicate reply arrives a beat later. If the shell is sitting idle at its
// prompt by then (e.g. after a reconnect, whose first-frame jiggle-resize fires
// a SIGWINCH that makes the prompt re-probe the terminal), the duplicate is
// "typed" at the prompt as literal garbage like:
//
//   ]10;rgb:ffff/ffff/ffff]11;rgb:0101/0101/0000[?1;2c
//
// OSC 10 (default foreground), OSC 11 (default background, which here echoes the
// app `--background` we hand xterm as its theme), and the Primary DA `[?1;2c`
// (xterm.js's hardcoded "VT100 with Advanced Video Option" answer).
//
// The fix: make the viewer stop auto-answering these queries so the PTY only
// ever sees one set of replies (dux-core's) plus genuine user input. Custom
// parser handlers are tried before xterm's built-in ones, and returning `true`
// marks the sequence handled so the built-in responder never runs. None of the
// suppressed sequences change what is rendered (they are pure reports), so the
// viewer's display is unaffected.
export function suppressViewerReports(term: Terminal): void {
  // Always report "handled" so xterm's built-in responder is skipped.
  const swallow = () => true

  // Device Attributes. Pure capability queries, no display effect:
  //   Primary   CSI c   / CSI 0 c
  //   Secondary CSI > c / CSI > 0 c
  //   Tertiary  CSI = c
  term.parser.registerCsiHandler({ final: "c" }, swallow)
  term.parser.registerCsiHandler({ prefix: ">", final: "c" }, swallow)
  term.parser.registerCsiHandler({ prefix: "=", final: "c" }, swallow)

  // Device Status Report / cursor-position report (CSI 5 n, CSI 6 n) and the
  // DEC private form (CSI ? n). A prompt that probes cursor position would
  // otherwise get a duplicate report typed back at it just like the colors.
  term.parser.registerCsiHandler({ final: "n" }, swallow)
  term.parser.registerCsiHandler({ prefix: "?", final: "n" }, swallow)

  // OSC 10/11/12 default foreground/background/cursor color. Only the QUERY
  // form (`OSC 10 ; ? ST`, data === "?") produces a reply, so suppress just
  // that and let a SET (a real color value) fall through to xterm's built-in so
  // the viewer still applies a child-requested recolor.
  const swallowColorQuery = (data: string) => data === "?"
  term.parser.registerOscHandler(10, swallowColorQuery)
  term.parser.registerOscHandler(11, swallowColorQuery)
  term.parser.registerOscHandler(12, swallowColorQuery)
}
