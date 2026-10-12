// The editor's status bar, as pure decisions: what it reads off Monaco, how it
// words each item, and which items a given state carries. An item that does not
// apply to what is on screen is absent rather than blank, so the bar never shows
// a cursor position for a picture or a size it has not been told.

// The slice of Monaco's code editor the bar reads. Structural, so this module
// never imports the Monaco bundle and the tests can hand it a stand-in.
export interface EditorStatusSource {
  getModel(): {
    getLineCount(): number
    getEOL(): string
    getOptions(): { insertSpaces: boolean; tabSize: number; indentSize: number }
    getValueLengthInRange(range: never): number
  } | null
  getPosition(): { lineNumber: number; column: number } | null
  getSelection(): { isEmpty(): boolean } | null
}

export interface Indentation {
  kind: "spaces" | "tabs"
  size: number
}

export interface EditorCursorStatus {
  line: number
  column: number
  // Characters the selection covers, 0 when there is none.
  selected: number
  lineCount: number
  indentation: Indentation
  eol: "LF" | "CRLF"
}

// What the editor reports right now. The indentation is the MODEL's, read after
// Monaco's own detection has run on the file, not the option dux passes in, so a
// tab-indented Makefile says so.
export function readEditorStatus(
  editor: EditorStatusSource,
): EditorCursorStatus | null {
  const model = editor.getModel()
  if (model === null) return null
  const position = editor.getPosition()
  const selection = editor.getSelection()
  const options = model.getOptions()
  return {
    line: position?.lineNumber ?? 1,
    column: position?.column ?? 1,
    selected:
      selection === null || selection.isEmpty()
        ? 0
        : model.getValueLengthInRange(selection as never),
    lineCount: model.getLineCount(),
    indentation: options.insertSpaces
      ? { kind: "spaces", size: options.indentSize }
      : { kind: "tabs", size: options.tabSize },
    eol: eolLabel(model.getEOL()),
  }
}

export function cursorLabel(status: EditorCursorStatus): string {
  const at = `Ln ${status.line}, Col ${status.column}`
  return status.selected > 0 ? `${at} (${status.selected} selected)` : at
}

export function lineCountLabel(count: number): string {
  return `${count.toLocaleString("en-US")} ${count === 1 ? "line" : "lines"}`
}

export function indentationLabel(indentation: Indentation): string {
  return `${indentation.kind === "spaces" ? "Spaces" : "Tabs"}: ${indentation.size}`
}

export function eolLabel(eol: string): "LF" | "CRLF" {
  return eol === "\r\n" ? "CRLF" : "LF"
}

const UNITS = ["KiB", "MiB", "GiB", "TiB"] as const

// Compact on purpose: the file info panel spells out the exact byte count, and a
// status bar item has room for one short figure.
export function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  let value = bytes / 1024
  let unit: string = UNITS[0]
  for (let i = 1; i < UNITS.length && value >= 1024; i += 1) {
    value /= 1024
    unit = UNITS[i]
  }
  return `${value.toFixed(1)} ${unit}`
}

export interface ImageDimensions {
  path: string
  width: number
  height: number
}

export interface StatusBarInput {
  tab: { path: string; mode: "file" | "diff" } | null
  dirty: boolean
  readOnly: boolean
  image: boolean
  // The last image that finished loading, tagged with its path so a picture
  // still loading never borrows the previous one's size.
  imageDims: ImageDimensions | null
  fileReady: boolean
  binary: boolean
  preview: boolean
  // The diff editor is what is on screen: a two-sided diff, not a patch head,
  // a binary refusal or a preview.
  diffEditorShown: boolean
  // The size on disk from the last read or save, null when none is known.
  size: number | null
  // The language registry has loaded, so the picker has rows to offer.
  languageAvailable: boolean
  // The status bar's name for the file's encoding, plain UTF-8 included, or
  // null when no read has said.
  encodingLabel: string | null
  // The file's line endings are mixed or include a lone CR.
  mixedEol: boolean
  // Why a read-only file is read-only, when it is for a reason of its own
  // (its encoding cannot write it back unchanged); null for the usual ones.
  readOnlyReason: string | null
}

export interface StatusBarPlan {
  path: string | null
  dirty: boolean
  readOnly: boolean
  size: number | null
  dimensions: { width: number; height: number } | null
  // The items Monaco's code editor reports: cursor, line count, indentation and
  // line endings. Only while it is the thing on screen.
  editing: boolean
  language: boolean
  // The encoding item, by its label: any text file in file mode. Never in
  // diff mode, which shows two versions that need not share
  // one, and never for a picture or a binary file, which have no text.
  encoding: string | null
  // Say "Mixed" in place of the editor's own line ending style, which is
  // only what a save would write.
  mixedEol: boolean
  readOnlyReason: string | null
}

const EMPTY_PLAN: StatusBarPlan = {
  path: null,
  dirty: false,
  readOnly: false,
  size: null,
  dimensions: null,
  editing: false,
  language: false,
  encoding: null,
  mixedEol: false,
  readOnlyReason: null,
}

export function planStatusBar(input: StatusBarInput): StatusBarPlan {
  const { tab } = input
  if (tab === null) return EMPTY_PLAN
  if (input.image) {
    const dims =
      input.imageDims !== null && input.imageDims.path === tab.path
        ? { width: input.imageDims.width, height: input.imageDims.height }
        : null
    return { ...EMPTY_PLAN, path: tab.path, dimensions: dims }
  }
  if (tab.mode === "diff") {
    // A diff is two versions of the file, so a single size or a single cursor
    // would be a claim about only one of them. The picker stays because the
    // diff editor highlights by it.
    return {
      ...EMPTY_PLAN,
      path: tab.path,
      dirty: input.dirty,
      language: input.diffEditorShown && input.languageAvailable,
    }
  }
  const editing = input.fileReady && !input.binary && !input.preview
  return {
    ...EMPTY_PLAN,
    path: tab.path,
    dirty: input.dirty,
    readOnly: input.readOnly,
    size: input.fileReady ? input.size : null,
    editing,
    language: editing && input.languageAvailable,
    // The preview still shows this file's text, decoded this way, and a save
    // from it still writes it this way, so the item stays with it.
    encoding: input.fileReady && !input.binary ? input.encodingLabel : null,
    mixedEol: editing && input.mixedEol,
    readOnlyReason: input.readOnly ? input.readOnlyReason : null,
  }
}
