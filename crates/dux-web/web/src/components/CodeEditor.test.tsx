// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { useEffect } from "react"

// The path the wrapper's model belongs to right now: the wrapper swaps the
// model when its `path` prop changes.
let currentPath = ""
// Every change made to the model, in order, so a test can read what went into
// one undo step and in which order.
const modelCalls: string[] = []
// A model that keeps one line ending style, the way Monaco's does.
const model = {
  get uri() {
    return { toString: () => currentPath }
  },
  eol: "\n",
  text: "",
  getEOL() {
    return this.eol
  },
  getValue() {
    return this.text
  },
  getFullModelRange() {
    return { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 }
  },
  setEOL: vi.fn(function (this: { eol: string }, seq: number) {
    modelCalls.push(`setEOL:${seq}`)
    this.eol = seq === 1 ? "\r\n" : "\n"
  }),
  pushEOL: vi.fn(function (this: { eol: string; text: string; emits: boolean }, seq: number) {
    modelCalls.push(`pushEOL:${seq}`)
    this.eol = seq === 1 ? "\r\n" : "\n"
    this.text = this.text.replace(/\r?\n/g, this.eol)
    // Monaco reports a change through whatever handler the wrapper holds.
    if (this.emits) handlers.at(-2)?.(this.text, { isFlush: false } as never)
  }),
  emits: false,
  pushStackElement: vi.fn(() => {
    modelCalls.push("pushStackElement")
  }),
  pushEditOperations: vi.fn(function (
    this: { text: string },
    _before: unknown,
    edits: Array<{ text: string }>,
  ) {
    modelCalls.push("pushEditOperations")
    this.text = edits[0].text
    if ((this as unknown as { emits: boolean }).emits)
      handlers.at(-2)?.(this.text, { isFlush: false } as never)
    return null
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
  Uri: { parse: (p: string) => ({ toString: () => p }) },
}

vi.mock("@/lib/monacoSetup", () => ({ monaco: fakeMonaco }))
// The options the wrapper was last handed, so a test can read what Monaco got.
let lastOptions: Record<string, unknown> = {}
// Every change handler the wrapper was handed, in render order.
const handlers: Array<(v: string | undefined) => void> = []
vi.mock("@monaco-editor/react", () => ({
  Editor: ({
    onMount,
    options,
    path,
    onChange,
  }: {
    onMount: (ed: unknown, mon: unknown) => void
    options: Record<string, unknown>
    path: string
    onChange: (v: string | undefined) => void
  }) => {
    lastOptions = options
    currentPath = path
    handlers.push(onChange)
    // eslint-disable-next-line react-hooks/exhaustive-deps
    useEffect(() => onMount(fakeEditor, fakeMonaco), [])
    return null
  },
}))

describe("a change from the editor", () => {
  afterEach(() => {
    cleanup()
    handlers.length = 0
  })

  /// The wrapper can emit a change for the NEW file (a read-only setValue on
  /// a tab switch) through the handler it was given for the OLD one. A change
  /// is only ever about the model that produced it, so it never lands on the
  /// other file's draft.
  /// The wrapper pushes a new value into a read-only editor with setValue,
  /// which Monaco reports as a flush, and Monaco's text can differ from what
  /// was pushed (it drops a leading U+FEFF). A flush is the editor's own doing,
  /// never a keystroke, so it never reaches the draft.
  it("never passes on a flush, which is the editor setting its own value", async () => {
    const { default: CodeEditor } = await import("./CodeEditor")
    const onChange = vi.fn()
    render(<CodeEditor path="a.txt" value="x" onSave={() => {}} onChange={onChange} />)
    const handler = handlers.at(-1)! as (v: string, ev?: { isFlush: boolean }) => void
    handler("x stripped", { isFlush: true })
    expect(onChange).not.toHaveBeenCalled()
    handler("typed", { isFlush: false })
    expect(onChange).toHaveBeenCalledWith("typed", "a.txt")
  })

  it("only reaches the file whose model produced it", async () => {
    const { default: CodeEditor } = await import("./CodeEditor")
    const onA = vi.fn()
    const onB = vi.fn()
    const props = { value: "x", onSave: () => {} }
    const view = render(<CodeEditor {...props} path="a.txt" onChange={onA} />)
    const aHandler = handlers.at(-1)!
    view.rerender(<CodeEditor {...props} path="b.txt" onChange={onB} readOnly />)
    aHandler("B's text")
    expect(onA).not.toHaveBeenCalled()
    expect(onB).not.toHaveBeenCalled()
    handlers.at(-1)!("typed into b")
    expect(onB).toHaveBeenCalledWith("typed into b", "b.txt")
  })
})

describe("a read-only file in the code editor", () => {
  afterEach(cleanup)

  /// Typing into a read-only buffer would be kept nowhere: it is not tracked
  /// as an edit, and closing the tab drops it without asking. Monaco itself
  /// must refuse the keystrokes, and say why.
  it("is read-only in Monaco, with the reason as its message", async () => {
    const { default: CodeEditor } = await import("./CodeEditor")
    render(
      <CodeEditor
        path="a.txt"
        value="x"
        onChange={() => {}}
        onSave={() => {}}
        readOnly
        readOnlyReason="Shift_JIS can't write this file back unchanged."
      />,
    )
    expect(lastOptions.readOnly).toBe(true)
    expect(lastOptions.readOnlyMessage).toEqual({
      value: "Shift_JIS can't write this file back unchanged.",
    })
  })

  it("is editable otherwise", async () => {
    const { default: CodeEditor } = await import("./CodeEditor")
    render(<CodeEditor path="a.txt" value="x" onChange={() => {}} onSave={() => {}} />)
    expect(lastOptions.readOnly).toBe(false)
  })
})

describe("the code editor's line ending style", () => {
  afterEach(() => {
    cleanup()
    model.eol = "\n"
    model.text = ""
    modelCalls.length = 0
    model.emits = false
  })

  // `reading` stands for the buffer's generation: a new one is a new reading
  // of the file (a load, a reload in place, a reopen).
  async function mount(eol: "LF" | "CRLF" | undefined, value = "a\nb\n") {
    const { default: CodeEditor } = await import("./CodeEditor")
    const onChange = vi.fn()
    const props = { path: "a.txt", onChange, onSave: () => {} }
    model.text = value
    const view = render(<CodeEditor {...props} value={value} eol={eol} reading={1} />)
    modelCalls.length = 0
    const rerender = (next: "LF" | "CRLF", nextValue = value, reading = 2) =>
      view.rerender(
        <CodeEditor {...props} value={nextValue} eol={next} reading={reading} />,
      )
    return { rerender, onChange }
  }

  /// A reload in place brings `a\r\nb\r\n` into a model holding `a\nb\n` in
  /// LF. Changing the style after the text was replaced (outside that undo
  /// step) leaves Monaco undoing the replacement with offsets from the other
  /// style, which put an extra blank line into the file. The style change and
  /// the text replacement are one undo step, style first, so the replacement
  /// is recorded in the style it is in, and the wrapper finds nothing left to
  /// push.
  it("changes the style and the text in one undo step, the style first", async () => {
    const { rerender } = await mount("LF")
    // The reload changes the text as well as its style.
    rerender("CRLF", "a\r\nb\r\nc\r\n")
    expect(modelCalls).toEqual([
      "pushStackElement",
      "pushEOL:1",
      "pushEditOperations",
      "pushStackElement",
    ])
    expect(model.getEOL()).toBe("\r\n")
    expect(model.getValue()).toBe("a\r\nb\r\nc\r\n")
  })

  /// The reload's own edits are the editor's doing, like the wrapper's: they
  /// never reach the draft, so a reload in place does not mark the tab
  /// unsaved.
  it("keeps its own edits out of the draft", async () => {
    const { rerender, onChange } = await mount("LF")
    model.emits = true
    rerender("CRLF", "a\r\nb\r\n")
    expect(onChange).not.toHaveBeenCalled()
  })

  /// Once a reading has been brought in, the style is the person's to change:
  /// an undo that takes the model back to LF is not fought back to CRLF.
  it("brings a reading's style in once, never against a later undo", async () => {
    const { rerender } = await mount("LF")
    rerender("CRLF", "a\r\nb\r\n", 2)
    modelCalls.length = 0
    // The person undid it: the model is LF again and the draft follows.
    model.eol = "\n"
    model.text = "a\nb\n"
    rerender("CRLF", "a\nb\n", 2)
    expect(modelCalls).toEqual([])
  })

  it("never changes the style outside the undo history", async () => {
    const { rerender } = await mount("LF")
    rerender("CRLF", "a\r\nb\r\n")
    expect(modelCalls.filter((c) => c.startsWith("setEOL"))).toEqual([])
  })

  it("leaves a model alone when nothing names a style, or it already holds it", async () => {
    await mount(undefined)
    expect(modelCalls).toEqual([])
    const { rerender } = await mount("LF")
    rerender("LF", "a\nb\nc\n")
    expect(modelCalls).toEqual([])
  })
})
