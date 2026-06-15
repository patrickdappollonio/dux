// Pure filename/extension parsing for language detection. Kept free of any Monaco
// import so it is unit-testable in node — `monacoSetup` pulls the multi-MB monaco
// bundle + workers and cannot load under vitest. `monacoSetup.monacoLanguageForPath`
// composes these with Monaco's language registry.

// The lowercased extension INCLUDING the leading dot (e.g. ".rs"), or "" when the
// filename has no extension. A leading-dot dotfile (".bashrc") counts as no
// extension — `dot > 0` excludes the leading position.
export function extensionForPath(path: string): string {
  const file = path.split("/").pop() ?? path
  const dot = file.lastIndexOf(".")
  return dot > 0 ? file.slice(dot).toLowerCase() : ""
}

// The bare filename (last path segment), lowercased — for matching Monaco
// `filenames` entries such as "Makefile"/"Dockerfile" case-insensitively.
export function fileNameForPath(path: string): string {
  return (path.split("/").pop() ?? path).toLowerCase()
}
