// Pure filename and extension parsing for language detection. Free of any
// Monaco import so it loads under vitest, which `monacoSetup` cannot.

// The lowercased extension including the leading dot, or "" when there is none.
// A leading-dot dotfile (".bashrc") counts as no extension.
export function extensionForPath(path: string): string {
  const file = path.split("/").pop() ?? path
  const dot = file.lastIndexOf(".")
  return dot > 0 ? file.slice(dot).toLowerCase() : ""
}

// The bare filename (last path segment), lowercased, so Monaco `filenames`
// entries such as "Makefile" match case-insensitively.
export function fileNameForPath(path: string): string {
  return (path.split("/").pop() ?? path).toLowerCase()
}
