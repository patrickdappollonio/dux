import { useEffect, useRef } from "react"
import { Editor, loader } from "@monaco-editor/react"
import type { editor } from "monaco-editor"
import * as monaco from "monaco-editor"
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker"
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker"
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker"
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker"
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker"

// Self-host Monaco from the bundle. dux serves the SPA offline from a single
// binary, so the wrapper's default CDN loader is never an option: point it at
// the bundled `monaco` instance, and supply the language workers via Vite
// `?worker` imports (each becomes a hashed chunk Vite emits into dist/ and
// rust-embed bakes into the binary). This whole module is lazy-loaded
// (React.lazy in EditorOverlay) so Monaco's weight never touches the main bundle.
self.MonacoEnvironment = {
  getWorker(_workerId, label) {
    switch (label) {
      case "json":
        return new jsonWorker()
      case "css":
      case "scss":
      case "less":
        return new cssWorker()
      case "html":
      case "handlebars":
      case "razor":
        return new htmlWorker()
      case "typescript":
      case "javascript":
        return new tsWorker()
      default:
        return new editorWorker()
    }
  },
}
loader.config({ monaco })

interface CodeEditorProps {
  // The worktree-relative path — Monaco infers the language from its extension.
  path: string
  value: string
  onChange: (value: string) => void
  onSave: () => void
}

export default function CodeEditor({
  path,
  value,
  onChange,
  onSave,
}: CodeEditorProps) {
  // Ctrl/Cmd+S is bound once on mount, but `onSave` is a fresh closure each
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
    ed.addCommand(mon.KeyMod.CtrlCmd | mon.KeyCode.KeyS, () => saveRef.current())
  }

  return (
    <Editor
      // The web UI is dark-only (main.tsx force-adds the `.dark` class), so a
      // fixed dark Monaco theme matches. If a light theme is ever added, derive
      // this from the documentElement class instead.
      theme="vs-dark"
      path={path}
      value={value}
      onChange={(v) => onChange(v ?? "")}
      onMount={handleMount}
      options={{
        fontSize: 13,
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        automaticLayout: true,
        tabSize: 2,
        renderWhitespace: "selection",
      }}
    />
  )
}
