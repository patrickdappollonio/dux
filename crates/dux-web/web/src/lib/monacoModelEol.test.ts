import { describe, expect, it } from "vitest"
import { holdNewModelsInLf } from "./monacoModelEol"

function fakeMonaco() {
  let listener: ((m: ReturnType<typeof model>) => void) | null = null
  const mon = {
    editor: {
      EndOfLineSequence: { LF: 0, CRLF: 1 },
      onDidCreateModel: (l: (m: ReturnType<typeof model>) => void) => {
        listener = l
      },
    },
  }
  return { mon, create: (m: ReturnType<typeof model>) => listener?.(m) }
}

function model(eol: string, lines: number) {
  return {
    eol,
    getEOL() {
      return this.eol
    },
    getLineCount: () => lines,
    setEOL(seq: number) {
      this.eol = seq === 0 ? "\n" : "\r\n"
    },
  }
}

describe("models made on a Windows browser", () => {
  /// Monaco gives a text with no line break the browser's platform style, and
  /// may restore an older model's history when it remakes one, so any model
  /// made with a default CRLF style is set to LF as it is made.
  it("holds a breakless model to LF, whatever its version", () => {
    const { mon, create } = fakeMonaco()
    holdNewModelsInLf(mon)
    const m = model("\r\n", 1)
    create(m)
    expect(m.getEOL()).toBe("\n")
  })

  it("leaves a model whose text has real CRLF breaks alone", () => {
    const { mon, create } = fakeMonaco()
    holdNewModelsInLf(mon)
    const m = model("\r\n", 3)
    create(m)
    expect(m.getEOL()).toBe("\r\n")
  })
})
