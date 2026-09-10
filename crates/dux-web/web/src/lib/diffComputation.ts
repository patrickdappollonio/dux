// Ask a Monaco diff widget whether its last diff computation was cut short.
//
// `getDiffComputationResult` is on the widget at runtime (it reads `quitEarly`
// straight off the diff state) but is absent from monaco-editor 0.55.1's public
// `editor.api.d.ts`, which exposes only `onDidUpdateDiff` and the deprecated
// `getLineChanges`. Hence the narrow structural read rather than a cast to a
// public type: an unknown answer is treated as "it finished", because a notice
// raised on a guess is worse than no notice.
interface DiffComputationReader {
  getDiffComputationResult?: () => { quitEarly?: boolean } | null
}

export function diffQuitEarly(editor: unknown): boolean {
  const reader = editor as DiffComputationReader | null
  if (typeof reader?.getDiffComputationResult !== "function") return false
  return reader.getDiffComputationResult()?.quitEarly === true
}
