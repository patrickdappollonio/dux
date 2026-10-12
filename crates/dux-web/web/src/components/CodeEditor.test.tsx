// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { useEffect } from "react"

// The path the wrapper's model belongs to right now: the wrapper swaps the
// model when its `path` prop changes.
let currentPath = ""
// Every change made to the model, in order.
const modelCalls: string[] = []
// A model that keeps one line ending style and a version, the way Monaco's
// does: a brand new model is version 1, and every edit moves it on.
const model = {
  get uri() {
    return { toString: () => currentPath }
  },
  eol: "\n",
  version: 1,
  getEOL() {
    return this.eol
  },
  getVersionId() {
    return this.version
  },
  setEOL: vi.fn(function (this: { eol: string }, seq: number) {
    modelCalls.push(`setEOL:${seq}`)
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
    model.version = 1
    modelCalls.length = 0
  })

  async function mount(lfText: boolean, path = "a.txt") {
    const { default: CodeEditor } = await import("./CodeEditor")
    const props = { onChange: () => {}, onSave: () => {}, value: "ab" }
    const view = render(<CodeEditor {...props} path={path} lfText={lfText} />)
    const rerender = (next: string) =>
      view.rerender(<CodeEditor {...props} path={next} lfText={lfText} />)
    return { rerender }
  }

  /// Monaco gives a model made from a text with no line break the platform's
  /// style (CRLF on Windows). The editor's text is always LF, so such a model
  /// is set to LF while it is brand new and has nothing to undo.
  it("makes a brand new model LF", async () => {
    model.eol = "\r\n"
    await mount(true)
    expect(modelCalls).toEqual(["setEOL:0"])
    expect(model.getEOL()).toBe("\n")
  })

  it("makes the model a file switch brings in LF too", async () => {
    const { rerender } = await mount(true)
    expect(modelCalls).toEqual([])
    model.eol = "\r\n"
    rerender("b.txt")
    expect(modelCalls).toEqual(["setEOL:0"])
  })

  /// Monaco's undo replays offsets in whatever style the model is in when it
  /// runs, so changing the style of a model with any history at all puts an
  /// undo or a redo out by a character per line. The file's own style lives
  /// in the buffer and is applied when a save writes.
  it("never changes the style of a model with an edit history", async () => {
    model.eol = "\r\n"
    model.version = 7
    const { rerender } = await mount(true)
    rerender("b.txt")
    expect(modelCalls).toEqual([])
  })

  it("leaves a model alone when the text was not handed over in LF", async () => {
    model.eol = "\r\n"
    await mount(false)
    expect(modelCalls).toEqual([])
  })
})
