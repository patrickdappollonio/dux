// The query tail every address may carry after its position: theater on a pane,
// and the editor's view of the file on screen. One splitter reads it and one
// builder writes it, so a theater tail and an editor tail can never be built
// separately and glued together.
//
//   #/agent/s1?view=theater
//   #/agent/s1/editor/file/notes.md?view=preview&line=12&top=4&lang=python&enc=Shift_JIS
//
// Only the keys below make a tail a query (see `QUERY_PART`).
// `view` is one key for both meanings: theater is only ever written on a pane's
// address and preview only on an editor's, so a position carries at most one.
// The first value of a repeated key is the one read, and when that first value
// is invalid the key is dropped rather than falling back to a later one.
//
// Pure, and free of the store, because the store reads it at module init to
// pick the boot shell.

import { REOPEN_CHOICES } from "./editorEncoding"
import { isImagePreviewPath, previewKind } from "./editorPreview"

// The editor's view of its active file, beyond which file and which mode. `line`
// is the cursor line and `top` the first visible line, both counted from 1: a
// scroll position as a line rather than pixels, so it means the same place on a
// screen of another size.
export interface EditorViewState {
  preview: boolean
  line: number | null
  top: number | null
  language: string | null
  encoding: string | null
}

export const DEFAULT_EDITOR_VIEW: EditorViewState = {
  preview: false,
  line: null,
  top: null,
  language: null,
  encoding: null,
}

// A tail is a query only when every `&`-separated part is `key=value` with a
// key dux writes. Anything else is part of the position: dux encodes every `?`
// it writes, so a raw one in some other shape is a hand-written path such as
// `a?b.md` or `a?b=c.md`. A known key with a value it cannot use still makes a
// query, and only that value is dropped.
const QUERY_PART = /^(?:view|line|top|lang|enc)=[^&]*$/

/**
 * Split the tail off a raw hash, before anything is decoded. The values stay
 * encoded, first value per key, so each field decodes its own and a malformed
 * escape costs only that field.
 */
export function splitRouteQuery(raw: string): {
  hash: string
  params: Map<string, string>
} {
  const at = raw.indexOf("?")
  const params = new Map<string, string>()
  if (at < 0) return { hash: raw, params }
  const parts = raw.slice(at + 1).split("&")
  if (!parts.every((part) => QUERY_PART.test(part))) return { hash: raw, params }
  for (const part of parts) {
    const eq = part.indexOf("=")
    const key = part.slice(0, eq)
    if (!params.has(key)) params.set(key, part.slice(eq + 1))
  }
  return { hash: raw.slice(0, at), params }
}

export function queryTheater(params: ReadonlyMap<string, string>): boolean {
  return params.get("view") === "theater"
}

function decoded(params: ReadonlyMap<string, string>, key: string): string | null {
  const raw = params.get(key)
  if (raw === undefined) return null
  try {
    return decodeURIComponent(raw)
  } catch {
    return null
  }
}

function lineNumber(value: string | null): number | null {
  return value !== null && /^[1-9][0-9]{0,8}$/.test(value) ? Number(value) : null
}

// The shape of a Monaco language id. Whether Monaco actually has the language
// is only known once its registry has loaded, so that check is the editor's.
const LANGUAGE_ID = /^[A-Za-z0-9._+#-]{1,64}$/

function languageId(value: string | null): string | null {
  return value !== null && LANGUAGE_ID.test(value) ? value : null
}

function encodingName(value: string | null): string | null {
  return value !== null && REOPEN_CHOICES.some((c) => c.encoding === value)
    ? value
    : null
}

/**
 * The view a tail describes for the file `path` in `mode`, in canonical form
 * (see `canonicalEditorView`).
 */
export function editorViewFromQuery(
  params: ReadonlyMap<string, string>,
  mode: "file" | "diff",
  path: string | null,
): EditorViewState | undefined {
  return canonicalEditorView(mode, path, {
    preview: params.get("view") === "preview",
    line: lineNumber(decoded(params, "line")),
    top: lineNumber(decoded(params, "top")),
    language: languageId(decoded(params, "lang")),
    encoding: encodingName(decoded(params, "enc")),
  })
}

/**
 * The canonical view: only what the file can carry, and `undefined` when that
 * is nothing at all, so an address with no view state and a route with none
 * are the same thing. A pathless editor carries nothing; a non-SVG image is
 * always its own preview and has no text, so it carries nothing either; preview
 * needs a file with a preview kind; the cursor and scroll exist in file mode
 * only. Every address the parser produces is in this form, and
 * parse(write(view)) == view holds over it.
 */
export function canonicalEditorView(
  mode: "file" | "diff",
  path: string | null,
  view: EditorViewState | undefined,
): EditorViewState | undefined {
  if (view === undefined || path === null || isImagePreviewPath(path)) return undefined
  const fileMode = mode === "file"
  const next: EditorViewState = {
    preview: view.preview && previewKind(path) !== null,
    line: fileMode ? view.line : null,
    top: fileMode ? view.top : null,
    language: view.language,
    encoding: view.encoding,
  }
  return sameEditorView(next, DEFAULT_EDITOR_VIEW) ? undefined : next
}

export function sameEditorView(
  a: EditorViewState | undefined,
  b: EditorViewState | undefined,
): boolean {
  const x = a ?? DEFAULT_EDITOR_VIEW
  const y = b ?? DEFAULT_EDITOR_VIEW
  return (
    x.preview === y.preview &&
    x.line === y.line &&
    x.top === y.top &&
    x.language === y.language &&
    x.encoding === y.encoding
  )
}

/**
 * Put the tail on a position. The home address never carries one. `view` is
 * written as given, so the caller hands over a canonical view.
 */
export function withRouteQuery(
  position: string,
  query: { theater: boolean; view?: EditorViewState },
): string {
  if (position === "") return position
  const parts: string[] = []
  const view = query.view
  if (query.theater) parts.push("view=theater")
  else if (view?.preview) parts.push("view=preview")
  if (view) {
    if (view.line !== null) parts.push(`line=${view.line}`)
    if (view.top !== null) parts.push(`top=${view.top}`)
    if (view.language !== null) parts.push(`lang=${encodeURIComponent(view.language)}`)
    if (view.encoding !== null) parts.push(`enc=${encodeURIComponent(view.encoding)}`)
  }
  return parts.length === 0 ? position : `${position}?${parts.join("&")}`
}
