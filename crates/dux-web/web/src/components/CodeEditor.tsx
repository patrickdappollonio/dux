import { useEffect, useRef } from "react"
import { Editor } from "@monaco-editor/react"
import type { editor } from "monaco-editor"
// Shared self-host bootstrap (workers + bundled monaco). Importing it runs the
// setup once for both the editor and the diff viewer.
import { monaco } from "@/lib/monacoSetup"
import { autoRevertLanguageId } from "@/lib/editorLanguage"

// The `monaco` instance's type, re-exported so a consumer (EditorOverlay's
// `EditorBody`, which owns tab lifecycle and disposes closed tabs' models) can
// type a ref to it WITHOUT importing `@/lib/monacoSetup` itself: that import
// runs the multi-MB self-host bootstrap eagerly, defeating the whole point of
// lazy-loading `CodeEditor`. A `MonacoInstance` type-only import is erased at
// build time, so it costs nothing.
export type MonacoInstance = typeof monaco

interface CodeEditorProps {
  // The worktree-relative path — Monaco infers the language from its extension.
  path: string
  // The user's per-file language override, from the header's language picker.
  // `undefined` means no override, which is the default: the `language` prop
  // is then absent and Monaco's own URI inference decides, exactly as before.
  // Changing it re-languages the live model, so a pick applies without a
  // remount and without touching the buffer. Setting a language is the
  // wrapper's own effect; CLEARING one is not (it skips an undefined value),
  // so the effect below owns that half.
  language?: string
  value: string
  onChange: (value: string) => void
  onSave: () => void
  // Fired once on mount with the `monaco` instance captured at `onMount`, so
  // the PARENT (EditorBody, which owns the tab lifecycle) can dispose a
  // closed tab's model by URI: `monaco.editor.getModel(monaco.Uri.parse(path))
  // ?.dispose()`. CodeEditor stays a pure active-tab renderer and never
  // disposes models itself, see EditorOverlay.tsx's "Monaco model lifecycle"
  // comment for the full contract.
  onReady?: (mon: MonacoInstance) => void
}

export default function CodeEditor({
  path,
  language,
  value,
  onChange,
  onSave,
  onReady,
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
  // Picking "Auto" clears the `language` prop, and @monaco-editor/react ignores
  // that: its effect is `model && language && setModelLanguage(...)`, so an
  // undefined value sets nothing and the model keeps the language the user is
  // clearing. Do it ourselves, for that one transition only; see
  // `autoRevertLanguageId` for why a path change is left alone and for the
  // shebang nuance this accepts.
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

  // Ctrl/Cmd+s is bound once on mount, but `onSave` is a fresh closure each
  // render (it reads the latest draft). Route the keybinding through a ref so it
  // always calls the current handler, never a stale one that saves old content.
  // The ref is updated in an effect (not during render) so re-renders stay pure.
  const saveRef = useRef(onSave)
  useEffect(() => {
    saveRef.current = onSave
  })

  function handleMount(
    ed: editor.IStandaloneCodeEditor,
    mon: typeof monaco,
  ): void {
    editorRef.current = ed
    monacoRef.current = mon
    ed.addCommand(mon.KeyMod.CtrlCmd | mon.KeyCode.KeyS, () => saveRef.current())
    onReady?.(mon)
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
      onChange={(v) => onChange(v ?? "")}
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
      }}
    />
  )
}
