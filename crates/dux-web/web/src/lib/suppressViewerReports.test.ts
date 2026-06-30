// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { Terminal } from "@xterm/xterm"

import { suppressViewerReports } from "./suppressViewerReports"

// Query sequences a child program / shell prompt emits to probe the terminal.
const OSC_FG_QUERY = "\x1b]10;?\x07"
const OSC_BG_QUERY = "\x1b]11;?\x07"
const OSC_CURSOR_QUERY = "\x1b]12;?\x07"
const DA_PRIMARY = "\x1b[c"
const DA_SECONDARY = "\x1b[>c"
const DSR_CURSOR = "\x1b[6n"

// Write `data` into the terminal and resolve once xterm has parsed it (its write
// callback fires after the chunk flushes). Any `onData` the parse triggers has
// already fired by then, so the captured array is complete when this resolves.
function writeAndDrain(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve))
}

describe("suppressViewerReports", () => {
  let terms: Terminal[] = []
  const make = () => {
    const t = new Terminal()
    terms.push(t)
    return t
  }
  afterEach(() => {
    for (const t of terms) t.dispose()
    terms = []
  })

  // Documents the underlying bug: a vanilla xterm.js viewer answers the child's
  // queries and emits the replies through `onData` (the same path as keystrokes),
  // which TerminalPane forwards back into the shared PTY. The Primary DA reply is
  // `[?1;2c`, exactly the `1;2c` users see typed at their shell prompt.
  //
  // Only the renderer-independent reports are asserted here: under jsdom there is
  // no canvas (HTMLCanvasElement.getContext is unimplemented), so xterm cannot
  // resolve a theme color and the OSC 10/11/12 color queries emit nothing in this
  // environment. In a real browser they DO reply (`]1x;rgb:...`), which is why the
  // user also sees those lines; the suppression below covers them by construction.
  it("vanilla xterm answers device/status queries via onData", async () => {
    const term = make()
    const out: string[] = []
    term.onData((d) => out.push(d))

    await writeAndDrain(term, DA_PRIMARY)
    await writeAndDrain(term, DA_SECONDARY)
    await writeAndDrain(term, DSR_CURSOR)

    const joined = out.join("")
    expect(joined).toContain("[?1;2c") // Primary DA, the reported `1;2c`
    expect(joined).toContain("[>") // Secondary DA
    expect(joined).toMatch(/\[\d+;\d+R/) // cursor-position report
  })

  it("suppresses every device/color query reply once installed", async () => {
    const term = make()
    suppressViewerReports(term)
    const out: string[] = []
    term.onData((d) => out.push(d))

    for (const q of [
      OSC_FG_QUERY,
      OSC_BG_QUERY,
      OSC_CURSOR_QUERY,
      DA_PRIMARY,
      DA_SECONDARY,
      DSR_CURSOR,
    ]) {
      await writeAndDrain(term, q)
    }

    expect(out.join("")).toBe("")
  })

  it("still forwards genuine user keystrokes", async () => {
    const term = make()
    suppressViewerReports(term)
    const out: string[] = []
    term.onData((d) => out.push(d))

    // Simulate typed input the way xterm surfaces it (keystrokes flow through
    // onData, not write); a plain write would only echo to the screen.
    term.input("ls\r", true)

    expect(out.join("")).toBe("ls\r")
  })

  it("lets an OSC 11 color SET through so the viewer still recolors", async () => {
    const term = make()
    suppressViewerReports(term)
    const out: string[] = []
    term.onData((d) => out.push(d))

    // A SET carries a real color value, not "?": it must not be swallowed as a
    // query, and it produces no reply of its own.
    await writeAndDrain(term, "\x1b]11;rgb:1234/1234/1234\x07")
    expect(out.join("")).toBe("")

    // A query AFTER the set is still suppressed (the set handler returning false
    // must not disarm suppression of the query form).
    await writeAndDrain(term, OSC_BG_QUERY)
    expect(out.join("")).toBe("")
  })
})
