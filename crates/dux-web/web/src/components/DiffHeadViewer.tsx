import { useEffect, useRef } from "react"
import { Editor, type OnMount } from "@monaco-editor/react"
import type { editor as MonacoEditor } from "monaco-editor"
// Importing the shared bootstrap wires Monaco's self-host (workers + bundled
// instance) before the editor mounts.
import "@/lib/monacoSetup"

interface DiffHeadViewerProps {
  // The head of git's own unified patch, already cut to length by the server.
  text: string
  // The sentence saying where the cut landed, or "" when nothing was cut.
  banner: string
}

// A read-only view of the head of git's own patch, for a file too large for the
// diff editor to hold. A plain text model with the `diff` language rather than
// the DiffEditor: there are no two sides here, only the patch git printed, and
// asking Monaco to diff a file this size is what the ceiling exists to avoid.
// Default export so it lazy-loads as its own chunk (see EditorBody).
export default function DiffHeadViewer({ text, banner }: DiffHeadViewerProps) {
  const modelRef = useRef<MonacoEditor.ITextModel | null>(null)
  const editorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null)
  const handleMount: OnMount = (editorInstance) => {
    editorRef.current = editorInstance
    modelRef.current = editorInstance.getModel()
  }
  useEffect(() => {
    return () => {
      const editor = editorRef.current
      const model = modelRef.current
      editorRef.current = null
      modelRef.current = null
      if (!model) return
      // Same ordering hazard as DiffViewer: React runs a deleted subtree's
      // cleanups parent-first, so detach the model before disposing it.
      try {
        editor?.setModel(null)
      } catch {
        // Widget already disposed; the model is still ours to reclaim.
      }
      if (!model.isDisposed()) model.dispose()
    }
  }, [])

  return (
    <div className="flex h-full min-h-0 flex-col">
      {banner !== "" && (
        <p
          className="border-b border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
          data-testid="diff-head-banner"
        >
          {banner}
        </p>
      )}
      <div className="min-h-0 flex-1">
        <Editor
          // The web UI is dark-only (main.tsx force-adds `.dark`).
          theme="vs-dark"
          value={text}
          language="diff"
          onMount={handleMount}
          keepCurrentModel
          options={{
            readOnly: true,
            fontSize: 14,
            lineHeight: 1.6,
            lineDecorationsWidth: 16,
            wordWrap: "on",
            minimap: { enabled: false },
            scrollBeyondLastLine: false,
            automaticLayout: true,
          }}
        />
      </div>
    </div>
  )
}
