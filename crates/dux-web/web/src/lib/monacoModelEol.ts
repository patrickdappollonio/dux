// The editor's text is always held with LF breaks (see `editorTextOf`), and a
// Monaco model takes its line ending style from its text when it is made: LF
// text gives an LF model. The exception is a text with no line break at all,
// which Monaco gives the BROWSER's platform default: CRLF on Windows. Such a
// model would then turn every Enter into CRLF and report its text in CRLF, so
// the draft would drift from the LF the rest of the editor assumes. Monaco can
// also bring back an earlier model's version and undo history when it remakes
// one for the same file, so a check on a model being brand new cannot catch it.
//
// So every model is held to LF the moment it is made, whoever makes it, on
// every platform. Only a model with a single line is changed: its style is a
// default rather than a fact about the text, and changing it rewrites no text,
// so no undo step recorded against it can be thrown out. A model with real
// CRLF breaks (a diff side showing a CRLF file) is left as its text made it.
// Free of any Monaco import so it loads under vitest.

interface ModelLike {
  getEOL(): string
  getLineCount(): number
  setEOL(eol: number): void
}

export interface MonacoEolLike {
  editor: {
    EndOfLineSequence: { LF: number }
    onDidCreateModel(listener: (model: ModelLike) => void): unknown
  }
}

export function holdNewModelsInLf(mon: MonacoEolLike): void {
  mon.editor.onDidCreateModel((model) => {
    if (model.getEOL() !== "\n" && model.getLineCount() === 1) {
      model.setEOL(mon.editor.EndOfLineSequence.LF)
    }
  })
}
