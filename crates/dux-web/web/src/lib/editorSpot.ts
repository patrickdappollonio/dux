// Where the person is in a file: the cursor line and the first visible line,
// read off the live editor when a link to this view is taken, and gone to when
// such a link is opened. Structural, so Monaco's editor fits it and a test can
// fake it.

export interface SpotEditor {
  getModel(): { getLineCount(): number } | null
  getPosition(): { lineNumber: number } | null
  getVisibleRanges(): readonly { startLineNumber: number }[]
  setPosition(position: { lineNumber: number; column: number }): void
  setScrollTop(top: number): void
  revealLineInCenter(line: number): void
  getTopForLineNumber(line: number): number
}

export interface EditorSpot {
  line: number | null
  top: number | null
}

export function readEditorSpot(editor: SpotEditor): EditorSpot {
  return {
    line: editor.getPosition()?.lineNumber ?? null,
    top: editor.getVisibleRanges()[0]?.startLineNumber ?? null,
  }
}

// A line past the end lands on the last one: the file may have shrunk since
// the link was taken.
export function goToEditorSpot(editor: SpotEditor, spot: EditorSpot): void {
  const last = Math.max(1, editor.getModel()?.getLineCount() ?? 1)
  const clamp = (line: number) => Math.min(line, last)
  if (spot.line !== null) {
    editor.setPosition({ lineNumber: clamp(spot.line), column: 1 })
  }
  if (spot.top !== null) {
    editor.setScrollTop(editor.getTopForLineNumber(clamp(spot.top)))
  } else if (spot.line !== null) {
    editor.revealLineInCenter(clamp(spot.line))
  }
}
