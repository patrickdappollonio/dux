import { useEffect, useMemo, useRef } from "react"
import { DiffEditor, type DiffOnMount } from "@monaco-editor/react"
import type { editor as MonacoEditor } from "monaco-editor"
// Importing the shared bootstrap wires Monaco's self-host (workers + bundled
// instance) before DiffEditor mounts, and gives us the path→language helper.
import { monacoLanguageForPath } from "@/lib/monacoSetup"

interface DiffViewerProps {
  // Worktree-relative path — used only to pick the syntax language for both sides.
  path: string
  // File content at HEAD ("" for an added file → all-insert diff).
  original: string
  // Working-copy content ("" for a deleted file → all-delete diff).
  modified: string
}

// Read-only INLINE (unified) diff of HEAD vs the working copy, rendered with
// Monaco's DiffEditor (`renderSideBySide: false`) — interleaved red/green rows in
// one column, like the previous diff view, so the file tree stays visible without
// cramping. `readOnly` still permits selection + copy, which is the whole point of
// viewing a diff here; the colors are Monaco's built-in diff styling under the
// dark theme. Default export so it lazy-loads as its own chunk (see EditorOverlay).
export default function DiffViewer({ path, original, modified }: DiffViewerProps) {
  // Path only changes when the user switches files; memoize so the language scan
  // doesn't repeat on every parent re-render.
  const language = useMemo(() => monacoLanguageForPath(path), [path])

  // The two TextModels the diff widget holds, captured on mount so we can
  // dispose them ourselves on unmount. We MUST manage them because @monaco-
  // editor/react's own unmount cleanup disposes the models BEFORE it disposes
  // the DiffEditor widget, and Monaco 0.55 asserts on a model disposed while
  // still set on a live widget ("TextModel got disposed before DiffEditorWidget
  // model got reset"), re-throwing it asynchronously as console noise on every
  // diff-view close. Passing keepCurrent{Original,Modified}Model below tells the
  // library to leave the models alone (it then only disposes the widget), which
  // silences the assertion but would LEAK the anonymous in-memory models; this
  // cleanup reclaims them. Ordering is NOT safe on its own: React runs a
  // deleted subtree's cleanups parent-first, so this cleanup fires while the
  // child's widget is still live and still holds the models (see the detach
  // step inside the cleanup). `onMount` fires after the wrapper's internal model-swap effects, so
  // getModel() here is the exact stable pair the widget keeps for its lifetime
  // (path/content/language changes reuse these models via setValue, never
  // replace them, since we pass no model paths).
  const modelsRef = useRef<MonacoEditor.IDiffEditorModel | null>(null)
  const editorRef = useRef<MonacoEditor.IStandaloneDiffEditor | null>(null)
  const handleMount: DiffOnMount = (editorInstance) => {
    editorRef.current = editorInstance
    modelsRef.current = editorInstance.getModel()
  }
  useEffect(() => {
    return () => {
      const editor = editorRef.current
      const models = modelsRef.current
      editorRef.current = null
      modelsRef.current = null
      if (!models) return
      // On unmount this component's subtree is DELETED, and React runs a
      // deleted subtree's cleanups PARENT-FIRST: this cleanup runs while the
      // library's DiffEditor child is still live and its widget still holds
      // these models. Monaco 0.55 asserts on disposing a TextModel in exactly
      // that state (measured in the preview env: "TextModel got disposed
      // before DiffEditorWidget model got reset" thrown during the commit
      // that swaps the diff arm for the loading arm on a preview-replace).
      // So detach the pair from the widget first, then dispose. The catch
      // covers a widget the library already disposed (nothing left to
      // detach); the models are still ours to reclaim.
      try {
        editor?.setModel(null)
      } catch {
        // Widget already disposed; fall through to model disposal.
      }
      if (!models.original.isDisposed()) models.original.dispose()
      if (!models.modified.isDisposed()) models.modified.dispose()
    }
  }, [])

  return (
    <DiffEditor
      // The web UI is dark-only (main.tsx force-adds `.dark`), matching vs-dark.
      theme="vs-dark"
      original={original}
      modified={modified}
      language={language}
      onMount={handleMount}
      // Leave the models to us on unmount (see modelsRef cleanup above): the
      // library otherwise disposes them before the widget, tripping Monaco's
      // "TextModel got disposed before DiffEditorWidget model got reset".
      keepCurrentOriginalModel
      keepCurrentModifiedModel
      options={{
        readOnly: true,
        // The original side is always read-only; be explicit so a future Monaco
        // default change can't make it editable.
        originalEditable: false,
        // Interleaved (unified) rather than two side-by-side panes — keeps the
        // file tree's space and matches the old diff's single-column layout.
        renderSideBySide: false,
        fontSize: 14,
        lineHeight: 1.6,
        // Breathing room between the line-number gutter and the code so the text
        // isn't flush against the numbers. The +/- line background still fills the
        // row; this only insets the text (Monaco default is a cramped ~10px).
        lineDecorationsWidth: 16,
        wordWrap: "on",
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        automaticLayout: true,
        // Hide the inline change-accept arrows — this is a viewer, not a merge UI.
        renderMarginRevertIcon: false,
      }}
    />
  )
}
