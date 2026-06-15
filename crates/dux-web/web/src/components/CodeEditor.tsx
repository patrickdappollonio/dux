import { useEffect, useRef } from "react"
import { Editor } from "@monaco-editor/react"
import type { editor } from "monaco-editor"
// Shared self-host bootstrap (workers + bundled monaco). Importing it runs the
// setup once for both the editor and the diff viewer.
import { monaco } from "@/lib/monacoSetup"

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
