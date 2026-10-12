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
  // The text is held with LF breaks (a read named the file's style, which the
  // buffer keeps apart and a save applies). The model's text is then read as
  // LF whatever style it reports, so a model on a Windows browser can never
  // turn its breaks into an edit. Every model is made in LF anyway (see
  // `holdNewModelsInLf`); this is the second line of that defence.
  lfText?: boolean
  // The buffer cannot be saved (a symlink, a .git path, or an encoding that
  // cannot write the file back unchanged). Monaco then refuses keystrokes
  // outright: typed text would be kept nowhere, and closing the tab would
  // drop it without asking. `readOnlyReason` is what Monaco says when
  // someone tries.
  readOnly?: boolean
  readOnlyReason?: string
}

// The model's text as the buffer holds it: LF when the buffer's text is LF,
// whatever style the model itself reports.
function modelText(
  model: editor.ITextModel,
  mon: typeof monaco,
  lfText: boolean,
): string {
  return lfText ? model.getValue(mon.editor.EndOfLinePreference.LF) : model.getValue()
}

export default function CodeEditor({
  path,
  language,
  value,
  onChange,
  onSave,
  onReady,
  onEditorChange,
  lfText = false,
  readOnly = false,
  readOnlyReason,
}: CodeEditorProps) {
  // The live editor + the monaco instance, captured at onMount. Needed for the
  // Auto revert below, which the wrapper cannot do for us.
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null)
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

  // Raised while this component edits the model itself, so that edit is never
  // taken for a keystroke.
  const programmaticRef = useRef(false)

  // The buffer is the truth, and the model on screen holds its text. Monaco
  // keeps a model per file after its tab is left, and the wrapper pushes
  // `value` only when the prop itself changes, so a model can come back on
  // screen holding an older reading: a reopen or reload that landed while its
  // tab was in the background, returning with text equal to the tab it
  // replaces, or a remount onto a retained model. Every time a model is put on
  // screen or the buffer's text changes, it is brought to the buffer's text
  // here, as one undo step of its own, before anyone can read or save it.
  //
  // It reads the latest render's buffer through a ref: the wrapper calls the
  // mount handler it was given on its FIRST render, which may be a buffer or
  // two old by the time Monaco has loaded.
  const bufferRef = useRef({ path, value, lfText })
  useLayoutEffect(() => {
    bufferRef.current = { path, value, lfText }
  })
  function bringModelToBuffer(
    ed: editor.IStandaloneCodeEditor | null,
    mon: typeof monaco | null,
  ): void {
    const { path, value, lfText } = bufferRef.current
    const model = ed?.getModel()
    if (!model || !mon) return
    if (model.uri.toString() !== mon.Uri.parse(path).toString()) return
    if (modelText(model, mon, lfText) === value) return
    programmaticRef.current = true
    try {
      model.pushStackElement()
      model.pushEditOperations(
        [],
        [{ range: model.getFullModelRange(), text: value, forceMoveMarkers: true }],
        () => null,
      )
      model.pushStackElement()
    } finally {
      programmaticRef.current = false
    }
  }

  // The wrapper's own effects (a model swap on a new path, its push of a new
  // value) run first, being a child's; this one checks what they left.
  useEffect(() => {
    bringModelToBuffer(editorRef.current, monacoRef.current)
  }, [path, value])

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
    bringModelToBuffer(ed, mon)
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
        // Nor this component's own edit bringing the model to the buffer.
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
        onChange(lfText && model && mon ? modelText(model, mon, true) : (v ?? ""), path)
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
