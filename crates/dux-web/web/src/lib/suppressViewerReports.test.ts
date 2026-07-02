// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { Terminal } from "@xterm/xterm"

import { isColorQuery, suppressViewerReports } from "./suppressViewerReports"

// Query sequences a child program / shell prompt emits to probe the terminal.
const OSC_FG_QUERY = "\x1b]10;?\x07"
const OSC_BG_QUERY = "\x1b]11;?\x07"
const OSC_CURSOR_QUERY = "\x1b]12;?\x07"
const OSC_PALETTE_QUERY = "\x1b]4;5;?\x07"
const DA_PRIMARY = "\x1b[c"
const DA_SECONDARY = "\x1b[>c"
const DSR_CURSOR = "\x1b[6n"
const DECRQM_BRACKETED_PASTE = "\x1b[?2004$p"
const DECRQSS_CONFORMANCE = '\x1bP$q"p\x1b\\'

// Write `data` into the terminal and resolve once xterm has parsed it (its write
// callback fires after the chunk flushes). Any `onData` the parse triggers has
// already fired by then, so the captured array is complete when this resolves.
function writeAndDrain(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve))
}

// The SET-vs-QUERY decision is pure logic and needs no terminal, so it is tested
// directly. This is what catches an inverted or over-broad predicate (e.g. one
// that swallowed color SETs too) regardless of the renderer environment.
describe("isColorQuery", () => {
  it("matches a query slot, not a color value", () => {
    expect(isColorQuery("?")).toBe(true) // OSC 10/11/12 query
    expect(isColorQuery("rgb:1234/1234/1234")).toBe(false) // OSC 10/11/12 set
    expect(isColorQuery("#ffffff")).toBe(false) // OSC set, hex form
    expect(isColorQuery("?;")).toBe(true) // trailing-semicolon variant
    expect(isColorQuery("?;?")).toBe(true) // stacked query
    expect(isColorQuery("5;?")).toBe(true) // OSC 4 indexed query
    expect(isColorQuery("5;rgb:1234/1234/1234")).toBe(false) // OSC 4 set
  })
})

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

  // Registration is asserted directly (independent of the renderer) so removing
  // a handler, or turning the whole function into a no-op, fails a test. This is
  // the only coverage for the OSC color handlers: xterm cannot fire an OSC color
  // reply in this environment (see the comment on the suppression test below),
  // so a behavioral assertion for them would pass vacuously.
  it("registers a swallowing handler for every query family", () => {
    const term = make()
    const csi = vi.spyOn(term.parser, "registerCsiHandler")
    const osc = vi.spyOn(term.parser, "registerOscHandler")
    const dcs = vi.spyOn(term.parser, "registerDcsHandler")

    suppressViewerReports(term)

    // OSC 4 (palette) + 10/11/12 (fg/bg/cursor) color queries.
    const oscIdents = osc.mock.calls.map((c) => c[0])
    expect(oscIdents).toEqual(expect.arrayContaining([4, 10, 11, 12]))
    // DA (c), DSR (n), and DECRQM ($p) go through CSI; DECRQSS ($q) through DCS.
    const csiFinals = csi.mock.calls.map((c) => c[0].final)
    expect(csiFinals).toEqual(expect.arrayContaining(["c", "n", "p"]))
    expect(dcs).toHaveBeenCalledWith(
      expect.objectContaining({ intermediates: "$", final: "q" }),
      expect.any(Function),
    )
  })

  // Documents the underlying bug: a vanilla xterm.js viewer answers the child's
  // queries and emits the replies through `onData` (the same path as keystrokes),
  // which TerminalPane forwards back into the shared PTY. The Primary DA reply is
  // `[?1;2c`, exactly the `1;2c` users see typed at their shell prompt.
  //
  // Only the renderer-independent reports are asserted here. The OSC 10/11/12
  // color queries are NOT, because xterm only initializes its theme service
  // inside `term.open()` (never called in this test), and the color-report path
  // returns early without it; in a real browser open() runs first and those
  // queries DO reply (`]1x;rgb:...`), which is why the user sees those lines.
  // DA/DSR/DECRQM/DECRQSS replies do not depend on the theme service, so they
  // fire here and give the suppression below something real to silence.
  it("vanilla xterm answers device/status queries via onData", async () => {
    const term = make()
    const out: string[] = []
    term.onData((d) => out.push(d))

    await writeAndDrain(term, DA_PRIMARY)
    await writeAndDrain(term, DA_SECONDARY)
    await writeAndDrain(term, DSR_CURSOR)
    await writeAndDrain(term, DECRQM_BRACKETED_PASTE)
    await writeAndDrain(term, DECRQSS_CONFORMANCE)

    const joined = out.join("")
    expect(joined).toMatch(/\[\?\d[\d;]*c/) // Primary DA (DEC private form)
    expect(joined).toContain("[>") // Secondary DA
    expect(joined).toMatch(/\[\d+;\d+R/) // cursor-position report
    expect(joined).toContain("$y") // DECRQM mode report
    expect(joined).toContain("$r") // DECRQSS status string
  })

  it("suppresses every device/status/color query reply once installed", async () => {
    const term = make()
    suppressViewerReports(term)
    const out: string[] = []
    term.onData((d) => out.push(d))

    for (const q of [
      OSC_FG_QUERY,
      OSC_BG_QUERY,
      OSC_CURSOR_QUERY,
      OSC_PALETTE_QUERY,
      DA_PRIMARY,
      DA_SECONDARY,
      DSR_CURSOR,
      DECRQM_BRACKETED_PASTE,
      DECRQSS_CONFORMANCE,
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

    // A query AFTER the set is still suppressed (the predicate returning false
    // for a set must not disarm suppression of the query form).
    await writeAndDrain(term, OSC_BG_QUERY)
    expect(out.join("")).toBe("")
  })
})
