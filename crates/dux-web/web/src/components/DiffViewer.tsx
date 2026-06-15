import { useMemo } from "react"
import { DiffEditor } from "@monaco-editor/react"
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
  return (
    <DiffEditor
      // The web UI is dark-only (main.tsx force-adds `.dark`), matching vs-dark.
      theme="vs-dark"
      original={original}
      modified={modified}
      language={language}
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
