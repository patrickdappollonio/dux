// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { useEffect } from "react"

// The path the wrapper's model belongs to right now: the wrapper swaps the
// model when its `path` prop changes.
let currentPath = ""
// Every change made to the model, in order.
const modelCalls: string[] = []
// A model that keeps its own text, the way a model Monaco retains for a file
// does after its tab is left: a later render's `value` reaches it only through
// the wrapper (which this stand-in leaves out) or through CodeEditor itself.
// `eol` is the style it reports its text in, as Monaco does by default.
const model = {
  get uri() {
    return { toString: () => currentPath }
  },
  eol: "\n",
  text: "",
  getEOL() {
    return this.eol
  },
  // Monaco's getValue(preference): 1 asks for LF, anything else the model's
  // own style.
  getValue(this: { text: string; eol: string }, preference?: number) {
    return preference === 1 ? this.text : this.text.replace(/\n/g, this.eol)
  },
  getFullModelRange() {
    return { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 }
  },
  pushStackElement: vi.fn(() => {
    modelCalls.push("pushStackElement")
  }),
  pushEditOperations: vi.fn(function (
    this: { text: string },
    _before: unknown,
    edits: Array<{ text: string }>,
  ) {
    modelCalls.push(`edit:${edits[0].text}`)
    this.text = edits[0].text
    // Monaco reports its own edit through whatever handler the wrapper holds.
    handlers.at(-1)?.(edits[0].text, { isFlush: false } as never)
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
  editor: {
    EndOfLineSequence: { LF: 0, CRLF: 1 },
    EndOfLinePreference: { TextDefined: 0, LF: 1, CRLF: 2 },
    setModelLanguage: () => {},
  },
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

describe("the model on screen holds the buffer's text", () => {
  afterEach(() => {
    cleanup()
    model.eol = "\n"
    model.text = ""
    modelCalls.length = 0
    handlers.length = 0
  })

  async function mount(path: string, value: string, onChange = vi.fn()) {
    const { default: CodeEditor } = await import("./CodeEditor")
    const props = { onChange, onSave: () => {}, lfText: true }
    const view = render(<CodeEditor {...props} path={path} value={value} />)
    const rerender = (nextPath: string, nextValue: string) =>
      view.rerender(<CodeEditor {...props} path={nextPath} value={nextValue} />)
    return { rerender, onChange }
  }

  /// Monaco keeps a model per file after its tab is left, and the wrapper only
  /// pushes `value` when the prop itself changes. A reopen that lands while
  /// the tab is in the background changes the buffer but not that model, and
  /// when the tab comes back with text equal to the tab it replaces, nothing
  /// pushes it: the screen shows the old reading while a save writes the new.
  it("brings a retained model to the buffer's text when its tab comes back", async () => {
    model.text = "cafÃ©"
    const { rerender, onChange } = await mount("b.txt", "cafÃ©")
    modelCalls.length = 0
    // A's retained model still holds what A showed before its reopen.
    model.text = "café"
    rerender("a.txt", "cafÃ©")
    expect(model.getValue(1)).toBe("cafÃ©")
    // One undo step of its own, and none of it is a keystroke.
    expect(modelCalls).toEqual(["pushStackElement", "edit:cafÃ©", "pushStackElement"])
    expect(onChange).not.toHaveBeenCalled()
  })

  it("brings a retained model to the buffer's text on a remount", async () => {
    model.text = "café"
    const { onChange } = await mount("a.txt", "cafÃ©")
    expect(model.getValue(1)).toBe("cafÃ©")
    expect(onChange).not.toHaveBeenCalled()
  })

  it("leaves a model alone when it already holds the text", async () => {
    model.text = "x\n"
    const { rerender } = await mount("a.txt", "x\n")
    model.text = "xy\n"
    rerender("a.txt", "xy\n")
    expect(modelCalls).toEqual([])
  })

  /// A model reporting CRLF (a browser on Windows) holds the same text: its
  /// breaks are compared, and passed on, as LF, never as an edit.
  it("reads a CRLF model's text as LF", async () => {
    model.eol = "\r\n"
    model.text = "a\nb\n"
    const { onChange } = await mount("a.txt", "a\nb\n")
    expect(modelCalls).toEqual([])
    model.text = "a\nb\nc\n"
    const handler = handlers.at(-1)! as (v: string, ev?: { isFlush: boolean }) => void
    handler("a\r\nb\r\nc\r\n", { isFlush: false })
    expect(onChange).toHaveBeenCalledWith("a\nb\nc\n", "a.txt")
  })
})
