import { useEffect, useMemo, useRef } from "react"
import { DiffEditor, type DiffOnMount } from "@monaco-editor/react"
import type { editor as MonacoEditor } from "monaco-editor"
// Importing the shared bootstrap wires Monaco's self-host (workers + bundled
// instance) before DiffEditor mounts, and gives us the path→language helper.
import { monacoLanguageForPath } from "@/lib/monacoSetup"
import { allDeleteDiffOptions } from "@/lib/diffPresentation"

interface DiffViewerProps {
  // Worktree-relative path, used only to pick the syntax language for both sides.
  path: string
  // The user's per-file language override, from the header's language picker.
  // When set it wins over the path-derived guess, for both sides of the diff:
  // a file the picker had to correct is just as wrong in diff mode.
  language?: string
  // File content at HEAD ("" for an added file → all-insert diff).
  original: string
  // Working-copy content ("" for a deleted file → all-delete diff).
  modified: string
  // True when this is an ALL-DELETE diff (isAllDeleteDiff, decided by the
  // caller who also carries the CSS marker class): drops the option-level
  // half of the phantom-inserted-line suppression (overview ruler + line
  // highlight, see allDeleteDiffOptions).
  allDelete?: boolean
}

// Read-only inline (unified) diff of HEAD against the working copy, rendered
// with Monaco's DiffEditor: interleaved rows in one column, so the file tree
// stays visible without cramping. `readOnly` still permits selection and copy,
// which is the point of viewing a diff here. Default export so it lazy-loads as
// its own chunk (see EditorOverlay).
export default function DiffViewer({
  path,
  language: languageOverride,
  original,
  modified,
  allDelete = false,
}: DiffViewerProps) {
  // Path only changes when the user switches files; memoize so the language scan
  // doesn't repeat on every parent re-render. An override skips the scan
  // entirely rather than being applied on top of it.
  const inferred = useMemo(() => monacoLanguageForPath(path), [path])
  const language = languageOverride ?? inferred

  // The two TextModels the diff widget holds, captured on mount so this
  // component disposes them itself. @monaco-editor/react's own cleanup disposes
  // the models before the widget, and Monaco 0.55 asserts on a model disposed
  // while still set on a live widget. The keepCurrent{Original,Modified}Model
  // props below stop the library touching them, which silences the assertion
  // but would leak the anonymous in-memory models. `onMount` fires after the
  // wrapper's model-swap effects, so getModel() is the stable pair the widget
  // keeps for its lifetime: content and language changes reuse these models via
  // setValue, since no model paths are passed.
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
      // React runs a deleted subtree's cleanups parent-first, so this runs
      // while the library's DiffEditor child is still live and its widget still
      // holds these models, and Monaco 0.55 asserts on disposing a TextModel in
      // that state. Detach the pair from the widget first, then dispose. The
      // catch covers a widget the library already disposed; the models are
      // still this component's to reclaim.
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
        // Interleaved (unified) rather than two side-by-side panes: keeps the
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
        // Hide the inline change-accept arrows: this is a viewer, not a merge UI.
        renderMarginRevertIcon: false,
        // All-delete diffs drop the overview ruler (a canvas, so CSS can't blank
        // its phantom green speck) and the current-line highlight (it borders
        // the phantom empty row). See allDeleteDiffOptions for the reasoning.
        ...allDeleteDiffOptions(allDelete),
      }}
    />
  )
}
