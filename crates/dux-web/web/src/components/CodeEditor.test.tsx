// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { useEffect } from "react"

// A model that keeps one line ending style, the way Monaco's does.
const model = {
  eol: "\n",
  getEOL() {
    return this.eol
  },
  setEOL: vi.fn(function (this: { eol: string }, seq: number) {
    this.eol = seq === 1 ? "\r\n" : "\n"
  }),
}
const fakeEditor = {
  getModel: () => model,
  addCommand: () => {},
}
const fakeMonaco = {
  KeyMod: { CtrlCmd: 0 },
  KeyCode: { KeyS: 0 },
  editor: { EndOfLineSequence: { LF: 0, CRLF: 1 }, setModelLanguage: () => {} },
  languages: { getLanguages: () => [] },
}

vi.mock("@/lib/monacoSetup", () => ({ monaco: fakeMonaco }))
vi.mock("@monaco-editor/react", () => ({
  Editor: ({ onMount }: { onMount: (ed: unknown, mon: unknown) => void }) => {
    // eslint-disable-next-line react-hooks/exhaustive-deps
    useEffect(() => onMount(fakeEditor, fakeMonaco), [])
    return null
  },
}))

describe("the code editor's line ending style", () => {
  afterEach(() => {
    cleanup()
    model.eol = "\n"
    model.setEOL.mockClear()
  })

  async function mount(eol: "LF" | "CRLF" | undefined) {
    const { default: CodeEditor } = await import("./CodeEditor")
    const props = {
      path: "a.txt",
      value: "a\nb\n",
      onChange: () => {},
      onSave: () => {},
    }
    const view = render(<CodeEditor {...props} eol={eol} />)
    return (next: "LF" | "CRLF") => view.rerender(<CodeEditor {...props} eol={next} />)
  }

  /// Text pushed into an existing model is rewritten to the model's style, so
  /// a reload that brings a CRLF file into an LF model would show CRLF in the
  /// notice and save LF. The model takes the style the read named.
  it("follows the style the read named, so a save writes what the notice says", async () => {
    const rerender = await mount("LF")
    expect(model.setEOL).not.toHaveBeenCalled()
    rerender("CRLF")
    expect(model.getEOL()).toBe("\r\n")
    expect(model.setEOL).toHaveBeenCalledWith(1)
  })

  it("leaves a model alone when nothing names a style", async () => {
    await mount(undefined)
    expect(model.setEOL).not.toHaveBeenCalled()
  })
})
