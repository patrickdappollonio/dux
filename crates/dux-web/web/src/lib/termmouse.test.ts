// @vitest-environment jsdom
import { describe, expect, it } from "vitest"

import {
  dispatchMouseReplay,
  latin1Bytes,
  rectCenter,
  tapReplaySteps,
  wheelReplaySteps,
} from "@/lib/termmouse"
import { installXtermMouseModel } from "@/lib/xtermMouseModel"
import { isDuxReplay } from "@/lib/termreplay"

// The link intercept sits in the CAPTURE phase on the pane container, which a
// replay dispatched at a descendant still travels through. Tagging is what
// tells dux's own events apart from the visitor's, since a jsdom (or an
// assistive technology) event is never `isTrusted`.
describe("dux-replay tagging", () => {
  it("marks every event a forwarded gesture dispatches", () => {
    const element = document.createElement("div")
    document.body.appendChild(element)
    const untagged: string[] = []
    for (const type of ["mousedown", "mouseup", "wheel"]) {
      element.addEventListener(type, (e) => {
        if (!isDuxReplay(e)) untagged.push(type)
      })
      document.addEventListener(type, (e) => {
        if (!isDuxReplay(e)) untagged.push(`doc:${type}`)
      })
    }
    dispatchMouseReplay(element, tapReplaySteps(), 5, 5)
    dispatchMouseReplay(element, wheelReplaySteps(1), 5, 5)
    expect(untagged).toEqual([])
    element.remove()
  })
})

describe("tapReplaySteps", () => {
  // A press at the element and a release at the DOCUMENT, in that order. xterm
  // arms its document-level mouseup listener from inside its own mousedown
  // handler, so a release dispatched at the element is never seen and a release
  // dispatched first has nothing listening for it.
  it("plans a left press at the element then a release at the document", () => {
    expect(tapReplaySteps()).toEqual([
      { type: "mousedown", target: "element", button: 0, buttons: 1 },
      { type: "mouseup", target: "document", button: 0, buttons: 0 },
    ])
  })
})

describe("wheelReplaySteps", () => {
  // Signed like Terminal.scrollLines: negative reveals OLDER output.
  it("uses a negative deltaY to reveal older output", () => {
    expect(wheelReplaySteps(-1)).toEqual([
      { type: "wheel", target: "element", button: 0, buttons: 0, deltaY: -1 },
    ])
  })

  it("uses a positive deltaY to reveal newer output", () => {
    expect(wheelReplaySteps(1)[0].deltaY).toBe(1)
  })

  it("plans one event per notch, never a bigger delta", () => {
    const steps = wheelReplaySteps(-3)
    expect(steps).toHaveLength(3)
    // The magnitude stays 1: xterm reads only the SIGN, and a larger delta in
    // one event would still be one report while risking the pixel branch.
    expect(steps.every((s) => s.deltaY === -1)).toBe(true)
  })

  it("plans nothing for a zero scroll", () => {
    expect(wheelReplaySteps(0)).toEqual([])
  })

  it("truncates a fractional notch count", () => {
    expect(wheelReplaySteps(-1.9)).toHaveLength(1)
  })
})

describe("rectCenter", () => {
  it("returns the middle of the rect as a client point", () => {
    expect(rectCenter({ left: 100, top: 50, width: 800, height: 480 })).toEqual({
      clientX: 500,
      clientY: 290,
    })
  })
})

describe("latin1Bytes", () => {
  // onBinary carries one byte per code unit. The X10 mouse encoding puts
  // `col + 32` in a byte, so column 64 and up leaves ASCII and TextEncoder
  // would emit the two-byte UTF-8 form, corrupting the report.
  it("keeps a high byte as ONE byte, where TextEncoder would emit two", () => {
    const s = "\x1b[M\xa0\xc8\x21"
    expect(Array.from(latin1Bytes(s))).toEqual([0x1b, 0x5b, 0x4d, 0xa0, 0xc8, 0x21])
    expect(new TextEncoder().encode(s).length).toBe(8)
  })

  it("round-trips the full byte range", () => {
    const all = Array.from({ length: 256 }, (_, i) => String.fromCharCode(i)).join("")
    expect(Array.from(latin1Bytes(all))).toEqual(
      Array.from({ length: 256 }, (_, i) => i),
    )
  })

  it("returns an empty array for an empty payload", () => {
    expect(latin1Bytes("").length).toBe(0)
  })
})

// End to end through the transcribed xterm pipeline (`lib/xtermMouseModel.ts`):
// the planners produce DOM events, the model resolves the cell and encodes, and
// the assertion is the bytes the APP would have received. One case per encoding
// xterm can actually be in, and the boundary cells of the grid.
describe("a replayed tap through xterm's pipeline", () => {
  // 80x24 of 10x20 cells at origin (100, 50), so the canvas is 800x480.
  const setup = (
    protocol: Parameters<
      ReturnType<typeof installXtermMouseModel>["setProtocol"]
    >[0],
    encoding: Parameters<
      ReturnType<typeof installXtermMouseModel>["setEncoding"]
    >[0],
    padding = { paddingLeft: 0, paddingTop: 0 },
  ) => {
    const element = document.createElement("div")
    const screen = document.createElement("div")
    element.appendChild(screen)
    document.body.appendChild(element)
    const rect = {
      left: 100,
      top: 50,
      right: 900,
      bottom: 530,
      width: 800,
      height: 480,
      x: 100,
      y: 50,
      toJSON() {},
    } as DOMRect
    element.getBoundingClientRect = () => rect
    screen.getBoundingClientRect = () => rect
    const data: string[] = []
    const binary: string[] = []
    const model = installXtermMouseModel({
      element,
      screen,
      cols: 80,
      rows: 24,
      cellWidth: 10,
      cellHeight: 20,
      ...padding,
      onData: (d) => data.push(d),
      onBinary: (d) => binary.push(d),
    })
    model.setProtocol(protocol)
    model.setEncoding(encoding)
    return { element, data, binary }
  }
  // The centre of the 1-based cell (col, row).
  const at = (col: number, row: number) => ({
    clientX: 100 + (col - 1) * 10 + 5,
    clientY: 50 + (row - 1) * 20 + 10,
  })
  const tap = (element: HTMLElement, p: { clientX: number; clientY: number }) =>
    dispatchMouseReplay(element, tapReplaySteps(), p.clientX, p.clientY)

  it("encodes SGR (?1006) as press and release at the cell", () => {
    const { element, data, binary } = setup("VT200", "SGR")
    tap(element, at(4, 3))
    expect(data).toEqual(["\x1b[<0;4;3M", "\x1b[<0;4;3m"])
    expect(binary).toEqual([])
  })

  it("encodes DEFAULT (X10 bytes) on the BINARY channel", () => {
    const { element, data, binary } = setup("VT200", "DEFAULT")
    tap(element, at(4, 3))
    expect(binary).toEqual(["\x1b[M \x24\x23", "\x1b[M\x23\x24\x23"])
    expect(data).toEqual([])
  })

  it("encodes SGR_PIXELS (?1016) in pixels rather than cells", () => {
    const { element, data } = setup("VT200", "SGR_PIXELS")
    tap(element, at(4, 3))
    expect(data).toEqual(["\x1b[<0;35;50M", "\x1b[<0;35;50m"])
  })

  // ?1005 (UTF-8) and ?1015 (urxvt) are deliberately absent: the installed
  // xterm parses both DECSETs and ignores them ("DECSET 1005 not supported"),
  // so it has no such state, and dux can never owe an app those bytes.
  it("has no UTF-8 or urxvt case, because xterm implements neither", () => {
    expect(() =>
      setup("VT200", "UTF8" as unknown as "SGR"),
    ).toThrow()
  })

  it("reports a press only under the X10 protocol", () => {
    const { element, binary } = setup("X10", "DEFAULT")
    tap(element, at(4, 3))
    expect(binary).toEqual(["\x1b[M \x24\x23"])
  })

  it("resolves the four boundary cells", () => {
    const { element, data } = setup("VT200", "SGR")
    for (const [c, r] of [
      [1, 1],
      [80, 1],
      [1, 24],
      [80, 24],
    ]) {
      tap(element, at(c, r))
    }
    expect(data.filter((d) => d.endsWith("M"))).toEqual([
      "\x1b[<0;1;1M",
      "\x1b[<0;80;1M",
      "\x1b[<0;1;24M",
      "\x1b[<0;80;24M",
    ])
  })

  it("clamps a point outside the canvas onto the edge cell", () => {
    const { element, data } = setup("VT200", "SGR")
    tap(element, { clientX: -1000, clientY: -1000 })
    tap(element, { clientX: 9999, clientY: 9999 })
    expect(data.filter((d) => d.endsWith("M"))).toEqual([
      "\x1b[<0;1;1M",
      "\x1b[<0;80;24M",
    ])
  })

  // The old parallel arithmetic divided the CONTAINER's width by the column
  // count and knew nothing about the screen element's CSS padding, so a padded
  // terminal landed the click a cell early. xterm subtracts the padding first.
  it("subtracts the screen element's padding, which the old cell math ignored", () => {
    const { element, data } = setup("VT200", "SGR", {
      paddingLeft: 8,
      paddingTop: 8,
    })
    // 8px into the padding is still the FIRST cell, not the last of nothing.
    tap(element, { clientX: 100 + 8 + 5, clientY: 50 + 8 + 10 })
    expect(data[0]).toBe("\x1b[<0;1;1M")
  })

  it("forwards a wheel notch as a single report, up and down", () => {
    const { element, data } = setup("DRAG", "SGR")
    const p = at(4, 3)
    dispatchMouseReplay(element, wheelReplaySteps(-1), p.clientX, p.clientY)
    dispatchMouseReplay(element, wheelReplaySteps(1), p.clientX, p.clientY)
    expect(data).toEqual(["\x1b[<64;4;3M", "\x1b[<65;4;3M"])
  })

  it("forwards no wheel report under X10, whose event mask has no wheel bit", () => {
    const { element, data, binary } = setup("X10", "DEFAULT")
    dispatchMouseReplay(element, wheelReplaySteps(-1), 200, 100)
    expect(data).toEqual([])
    expect(binary).toEqual([])
  })

  it("is a no-op with no terminal element", () => {
    expect(() => dispatchMouseReplay(null, tapReplaySteps(), 1, 1)).not.toThrow()
  })
})
