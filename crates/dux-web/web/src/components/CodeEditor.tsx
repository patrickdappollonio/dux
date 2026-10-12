import { useEffect, useLayoutEffect, useRef } from "react"
import { Editor } from "@monaco-editor/react"
import type { editor } from "monaco-editor"
// Shared self-host bootstrap (workers + bundled monaco). Importing it runs the
// setup once for both the editor and the diff viewer.
import { monaco } from "@/lib/monacoSetup"
import { autoRevertLanguageId } from "@/lib/editorLanguage"

// The `monaco` instance's type, re-exported so a consumer can type a ref to it
// without importing `@/lib/monacoSetup`, whose self-host bootstrap runs eagerly
// and would defeat lazy-loading `CodeEditor`. A type-only import is erased.
export type MonacoInstance = typeof monaco

interface CodeEditorProps {
  // The worktree-relative path: Monaco infers the language from its extension.
  path: string
  // The user's per-file language override. `undefined` means none, and Monaco's
  // own URI inference decides. Changing it re-languages the live model, so a pick
  // applies without a remount; the wrapper's effect sets a language but skips an
  // undefined value, so the effect below owns clearing one.
  language?: string
  value: string
  // Called with the new text and the path of the file it is the text of.
  onChange: (value: string, path: string) => void
  onSave: () => void
  // Fired once on mount with the `monaco` instance, so the parent, which owns
  // the tab lifecycle, can dispose a closed tab's model by URI. `CodeEditor`
  // stays a pure active-tab renderer and never disposes a model itself.
  onReady?: (mon: MonacoInstance) => void
  // The live editor, handed over on mount and taken back (null) on unmount, so
  // the status bar can follow its cursor and model without this component
  // knowing the bar exists.
  onEditorChange?: (ed: editor.IStandaloneCodeEditor | null) => void
  // The one line ending style the read named for this file, which the model
  // is held to. Undefined leaves Monaco's own pick alone.
  eol?: "LF" | "CRLF"
  // Which reading of the file `value` and `eol` come from (the buffer's
  // generation). The style is brought in once per reading: after that it is
  // the person's, and an undo back to the old style is not fought.
  reading?: number
  // The buffer cannot be saved (a symlink, a .git path, or an encoding that
  // cannot write the file back unchanged). Monaco then refuses keystrokes
  // outright: typed text would be kept nowhere, and closing the tab would
  // drop it without asking. `readOnlyReason` is what Monaco says when
  // someone tries.
  readOnly?: boolean
  readOnlyReason?: string
}

// Hold the model to `eol`, and bring in `value` with it when that is the
// change, as ONE undo step with the style changed first.
//
// Monaco rewrites text pushed into an existing model (a reload in place, a
// reopen) to the model's own style, so the model must take the style the read
// named or the notice, the status bar and a save disagree. Doing that after
// the text went in, outside its undo step, records the replacement in the old
// style and then changes the style underneath it, and undoing the replacement
// later applies offsets from the wrong style (a file came back with an extra
// blank line). So the style is pushed as an undoable change, then the text is
// replaced in the new style, both inside one stack element: an undo reverts
// both together, in the right order. Run before the editor wrapper pushes the
// value itself (it does so in a passive effect), so it finds nothing to do.
//
// Only for the model of this file: on a file switch the wrapper changes models
// itself, and the new model is fixed up once it is the one on screen.
//
// Returns whether it brought the style in (or found it there), so the caller
// records the reading as done. `programmatic` is raised for the edits it makes,
// which are the editor's own and never reach the draft.
function holdEol(
  ed: editor.IStandaloneCodeEditor | null,
  mon: typeof monaco | null,
  path: string,
  eol: "LF" | "CRLF" | undefined,
  value: string | undefined,
  programmatic: { current: boolean },
): boolean {
  const model = ed?.getModel()
  if (!model || !mon || eol === undefined) return false
  if (model.uri.toString() !== mon.Uri.parse(path).toString()) return false
  const want = eol === "CRLF" ? "\r\n" : "\n"
  if (model.getEOL() === want) return true
  programmatic.current = true
  try {
    applyEol(model, mon, eol, value)
  } finally {
    programmatic.current = false
  }
  return true
}

function applyEol(
  model: editor.ITextModel,
  mon: typeof monaco,
  eol: "LF" | "CRLF",
  value: string | undefined,
): void {
  model.pushStackElement()
  model.pushEOL(
    eol === "CRLF"
      ? mon.editor.EndOfLineSequence.CRLF
      : mon.editor.EndOfLineSequence.LF,
  )
  if (value !== undefined && model.getValue() !== value) {
    model.pushEditOperations(
      [],
      [{ range: model.getFullModelRange(), text: value }],
      () => null,
    )
  }
  model.pushStackElement()
}

export default function CodeEditor({
  path,
  language,
  value,
  onChange,
  onSave,
  onReady,
  onEditorChange,
  eol,
  reading,
  readOnly = false,
  readOnlyReason,
}: CodeEditorProps) {
  // The live editor + the monaco instance, captured at onMount. Needed for the
  // Auto revert below, which the wrapper cannot do for us.
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null)
  // Raised while this component edits the model itself (see `holdEol`).
  const programmaticRef = useRef(false)
  const monacoRef = useRef<typeof monaco | null>(null)
  // What the last render asked for, so the effect can see the TRANSITION and
  // not merely the current value.
  const lastLanguageRef = useRef<{ language?: string; path: string }>({
    language,
    path,
  })
  // Picking "Auto" clears the `language` prop, which @monaco-editor/react
  // ignores: an undefined value sets nothing and the model keeps the language
  // being cleared. Done here for that one transition; `autoRevertLanguageId` has
  // why a path change is left alone and the shebang nuance this accepts.
  useEffect(() => {
    const prev = lastLanguageRef.current
    const next = { language, path }
    lastLanguageRef.current = next
    const mon = monacoRef.current
    const model = editorRef.current?.getModel()
    if (!mon || !model) return
    const revertTo = autoRevertLanguageId(
      prev,
      next,
      mon.languages.getLanguages(),
    )
    if (revertTo !== null) mon.editor.setModelLanguage(model, revertTo)
  }, [language, path])

  // A layout effect: it runs before the wrapper's passive effect pushes the
  // value, so a reload that changes the style goes in as one undo step.
  //
  // Once per reading of a file: `broughtInRef` names the last path and
  // reading whose style is in the model, and nothing is done again for it,
  // so the person's own undo of a reload's style change stands.
  const broughtInRef = useRef<string | null>(null)
  const readingKey = `${path}\u0000${reading ?? ""}\u0000${eol ?? ""}`
  useLayoutEffect(() => {
    if (broughtInRef.current === readingKey) return
    if (holdEol(editorRef.current, monacoRef.current, path, eol, value, programmaticRef)) {
      broughtInRef.current = readingKey
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readingKey])
  // And again once the wrapper has switched to another file's model, which
  // only happens in its passive effect: that model is brought in here, as an
  // undoable style change of its own.
  useEffect(() => {
    if (broughtInRef.current === readingKey) return
    if (holdEol(editorRef.current, monacoRef.current, path, eol, undefined, programmaticRef)) {
      broughtInRef.current = readingKey
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readingKey])

  // The save chord is bound once on mount while `onSave` is a fresh closure each
  // render, so the binding goes through a ref or it saves old content. The ref
  // is updated in an effect, not during render, so re-renders stay pure.
  const saveRef = useRef(onSave)
  useEffect(() => {
    saveRef.current = onSave
  })

  // Through a ref for the same reason as `saveRef`, and so the unmount cleanup
  // below reaches the latest callback rather than the first one.
  const editorChangeRef = useRef(onEditorChange)
  useEffect(() => {
    editorChangeRef.current = onEditorChange
  })
  useEffect(() => {
    return () => editorChangeRef.current?.(null)
  }, [])

  function handleMount(
    ed: editor.IStandaloneCodeEditor,
    mon: typeof monaco,
  ): void {
    editorRef.current = ed
    monacoRef.current = mon
    ed.addCommand(mon.KeyMod.CtrlCmd | mon.KeyCode.KeyS, () => saveRef.current())
    onReady?.(mon)
    editorChangeRef.current?.(ed)
    if (holdEol(ed, mon, path, eol, undefined, programmaticRef)) {
      broughtInRef.current = `${path}\u0000${reading ?? ""}\u0000${eol ?? ""}`
    }
  }

  return (
    <Editor
      // The web UI is dark-only (main.tsx force-adds the `.dark` class), so a
      // fixed dark Monaco theme matches. If a light theme is ever added, derive
      // this from the documentElement class instead.
      theme="vs-dark"
      path={path}
      language={language}
      value={value}
      onChange={(v, ev) => {
        // A flush is Monaco replacing its whole value (the wrapper's setValue
        // into a read-only editor), the editor's own doing and never a
        // keystroke, and its text can differ from what was pushed (a leading
        // U+FEFF is dropped). Only keystrokes, undo and paste reach the draft.
        // The wrapper pushes into a writable editor with its change events
        // held back, so those never arrive here at all.
        if (ev?.isFlush) return
        // Nor our own reload edits (see `holdEol`): the draft already holds
        // that text, and the handler the wrapper holds at that moment may
        // be the previous render's, judging it against an old baseline.
        if (programmaticRef.current) return
        // A change is only ever about the model that produced it. On a tab
        // switch the wrapper can emit one for the new file's model (its
        // read-only setValue) through the handler it was given for the old
        // file, before it swaps the subscription; delivered, that writes one
        // file's text over another's draft. So a change whose model is not
        // this render's file is dropped, whatever the order inside the
        // wrapper.
        const model = editorRef.current?.getModel()
        const mon = monacoRef.current
        if (model && mon && model.uri.toString() !== mon.Uri.parse(path).toString()) {
          return
        }
        onChange(v ?? "", path)
      }}
      onMount={handleMount}
      options={{
        // 14px matches the app's text-sm body size; 13 read as too small.
        fontSize: 14,
        // A touch more than Monaco's default (~1.35–1.5×) for breathing room
        // between lines. Values below 8 are taken as a multiple of the font size
        // (8 and up are absolute pixels), so 1.6 → 1.6 × 14 ≈ 22px.
        lineHeight: 1.6,
        // Wrap long lines: Monaco keeps the line number on the first row, blanks
        // continuation rows, and indents wrapped text under the code (mirrors the
        // TUI diff wrapping). No horizontal scroll for overflowing lines.
        wordWrap: "on",
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        automaticLayout: true,
        tabSize: 2,
        renderWhitespace: "selection",
        readOnly,
        ...(readOnly && readOnlyReason
          ? { readOnlyMessage: { value: readOnlyReason } }
          : {}),
      }}
    />
  )
}
