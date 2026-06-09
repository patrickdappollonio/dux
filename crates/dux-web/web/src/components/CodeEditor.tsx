import { useEffect, useRef } from "react"
import { Editor, loader } from "@monaco-editor/react"
import type { editor } from "monaco-editor"
// `edcore.main` is the editor core + all editor contributions (find, folding,
// bracket matching, …) WITHOUT the typescript/json/css/html language services
// the default `monaco-editor` barrel registers. We then add only the Monarch
// GRAMMARS for syntax highlighting. The result ships just the editor worker — not
// the multi-MB language-service workers (`ts.worker` alone is ~6.6MB) — and drops
// the language-service client code from the editor chunk. Highlighting runs on
// the main thread; what we give up is IntelliSense/diagnostics, which add nothing
// for single-file worktree edits. This module is lazy-loaded (React.lazy in
// EditorOverlay) so even the trimmed Monaco never touches the main bundle.
import * as monaco from "monaco-editor/esm/vs/editor/edcore.main"
import "@/monacoLanguages"
// JSON has no "basic-language" grammar — its highlighting ships with the real
// JSON language service. We keep that one service (its worker is ~400KB vs the
// 6.6MB ts.worker we dropped) so JSON — the config-file format you edit most —
// gets a proper, well-tested tokenizer plus validation, instead of a hand-rolled
// grammar. No other language services are registered, so no other worker ships.
import "monaco-editor/esm/vs/language/json/monaco.contribution"
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker"
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker"

// Self-host: point the wrapper at the bundled `monaco` instance (no CDN — dux
// serves the SPA offline) and supply the workers via Vite `?worker` imports
// (hashed chunks rust-embed bakes into the binary).
self.MonacoEnvironment = {
  getWorker: (_id, label) =>
    label === "json" ? new jsonWorker() : new editorWorker(),
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
        // 14px matches the app's text-sm body size; 13 read as too small.
        fontSize: 14,
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
