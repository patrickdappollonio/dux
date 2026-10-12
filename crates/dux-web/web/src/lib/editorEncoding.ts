// The editor's text encodings, as pure decisions: what an encoding is called on
// screen, which ones the menus offer, what a save leaves the buffer in, and the
// per-file "this was a guess" notice the person has already dismissed.
//
// The server decides how a file decodes (a byte-order mark, else UTF-8, else
// windows-1252 as a flagged fallback) and answers with the WHATWG name it used.
// Nothing here guesses: the person is the one who can tell that a file is really
// Shift_JIS, and these lists are where they say so.

import { chip, prose, type Prose } from "./prose"

// What a save writes: the encoding's WHATWG name, and whether the bytes start
// with its byte-order mark.
export interface SaveEncoding {
  encoding: string
  bom: boolean
}

export const UTF8 = "UTF-8"

// What a read or a save of a plain file means. Also the answer for a buffer that
// predates encodings, which only ever held UTF-8.
export const PLAIN_UTF8: SaveEncoding = { encoding: UTF8, bom: false }

interface EncodingEntry {
  // The canonical WHATWG name, exactly as the server reports it.
  name: string
  label: string
}

// The encodings the menus offer: the Unicode ones first, then the legacy
// encodings people still meet in old source trees, grouped by script. Every
// name is one the server resolves; anything it reports outside this list is
// still labelled by its own name.
export const EDITOR_ENCODINGS: readonly EncodingEntry[] = [
  { name: "UTF-8", label: "UTF-8" },
  { name: "UTF-16LE", label: "UTF-16 LE" },
  { name: "UTF-16BE", label: "UTF-16 BE" },
  // The WHATWG decodes ISO-8859-1 as windows-1252, its superset, so the two
  // are one entry and the label says both.
  { name: "windows-1252", label: "Windows-1252 (ISO-8859-1)" },
  { name: "ISO-8859-15", label: "ISO-8859-15" },
  { name: "macintosh", label: "Mac Roman" },
  { name: "windows-1250", label: "Windows-1250 (Central European)" },
  { name: "ISO-8859-2", label: "ISO-8859-2 (Central European)" },
  { name: "windows-1251", label: "Windows-1251 (Cyrillic)" },
  { name: "KOI8-R", label: "KOI8-R (Cyrillic)" },
  { name: "KOI8-U", label: "KOI8-U (Cyrillic)" },
  { name: "IBM866", label: "CP866 (Cyrillic)" },
  { name: "windows-1253", label: "Windows-1253 (Greek)" },
  { name: "windows-1254", label: "Windows-1254 (Turkish)" },
  { name: "windows-1255", label: "Windows-1255 (Hebrew)" },
  { name: "windows-1256", label: "Windows-1256 (Arabic)" },
  { name: "windows-1257", label: "Windows-1257 (Baltic)" },
  { name: "windows-1258", label: "Windows-1258 (Vietnamese)" },
  { name: "windows-874", label: "Windows-874 (Thai)" },
  { name: "Shift_JIS", label: "Shift_JIS (Japanese)" },
  { name: "EUC-JP", label: "EUC-JP (Japanese)" },
  { name: "ISO-2022-JP", label: "ISO-2022-JP (Japanese)" },
  { name: "GBK", label: "GBK (Simplified Chinese)" },
  { name: "gb18030", label: "GB18030 (Simplified Chinese)" },
  { name: "Big5", label: "Big5 (Traditional Chinese)" },
  { name: "EUC-KR", label: "EUC-KR (Korean)" },
]

// The status bar's short names: the menus can afford the script in brackets,
// the bar cannot.
const SHORT_LABELS: Record<string, string> = {
  "windows-1252": "Windows-1252",
  macintosh: "Mac Roman",
  IBM866: "CP866",
  gb18030: "GB18030",
}

function entryLabel(name: string): string {
  return EDITOR_ENCODINGS.find((e) => e.name === name)?.label ?? name
}

// The name the status bar and the notice use: short, with the mark spelled out
// for UTF-8, where it is the only thing that differs from a plain file. UTF-16
// always has one when dux opened it by itself, so it goes unsaid there.
export function encodingLabel(encoding: string, bom: boolean): string {
  if (encoding === UTF8) return bom ? "UTF-8 with BOM" : "UTF-8"
  const short = SHORT_LABELS[encoding] ?? entryLabel(encoding).replace(/ \(.*\)$/, "")
  return short
}

export function isUtf16(encoding: string): boolean {
  return encoding === "UTF-16LE" || encoding === "UTF-16BE"
}

// The rows "Reopen with encoding" offers. A mark is read whatever was asked
// for, so there is no "with BOM" row here.
export const REOPEN_CHOICES: readonly { encoding: string; label: string }[] =
  EDITOR_ENCODINGS.map((e) => ({ encoding: e.name, label: e.label }))

// The rows "Save with encoding" offers. UTF-8 comes with and without a mark;
// UTF-16 always writes one, because a UTF-16 file without a mark would come
// back on the next open as a file full of NULs, which is to say binary.
export const SAVE_CHOICES: readonly (SaveEncoding & { label: string })[] = [
  { encoding: UTF8, bom: false, label: "UTF-8" },
  { encoding: UTF8, bom: true, label: "UTF-8 with BOM" },
  ...EDITOR_ENCODINGS.filter((e) => e.name !== UTF8).map((e) => ({
    encoding: e.name,
    bom: isUtf16(e.name),
    label: e.label,
  })),
]

export function sameSaveEncoding(a: SaveEncoding, b: SaveEncoding): boolean {
  return a.encoding === b.encoding && a.bom === b.bom
}

// What a buffer holds after a successful save in `saved`. An ordinary save keeps
// the buffer as it was; a conversion moves it to the new encoding and, because
// the person chose it, it is no longer a guess, and the next read asks for it by
// name (a Shift_JIS file has no mark to be recognised by).
export interface BufferEncodingFields {
  encoding: string
  bom: boolean
  encodingFallback: boolean
  encodingRequested: string | null
}

export function encodingAfterSave(
  prev: BufferEncodingFields,
  saved: SaveEncoding,
): BufferEncodingFields {
  if (sameSaveEncoding(saved, { encoding: prev.encoding, bom: prev.bom })) {
    return prev
  }
  return {
    encoding: saved.encoding,
    bom: saved.bom,
    encodingFallback: false,
    encodingRequested: saved.encoding,
  }
}

// The toast for a save refused because the text holds a character the encoding
// has no bytes for. It names the character as a chip with its code point (the
// character alone may be invisible, or look like one the encoding does have),
// says where it is, that nothing was written, and both ways forward. "Save with
// encoding" is the item's name in the status bar's encoding menu, which every
// screen carries.
export function unmappableNotice(
  path: string,
  refusal: { character: string; line: number; column: number; encoding: string },
): Prose {
  const codePoint = refusal.character.codePointAt(0)
  const hex =
    codePoint === undefined
      ? ""
      : ` (U+${codePoint.toString(16).toUpperCase().padStart(4, "0")})`
  return prose`Couldn't save ${chip(path)}: ${chip(refusal.character)}${hex} on line ${refusal.line}, column ${refusal.column} can't be written in ${encodingLabel(refusal.encoding, false)}, so nothing was written. Remove it, or use Save with encoding to convert the file to UTF-8.`
}

// --- The fallback notice -----------------------------------------------------
//
// Dismissed per file for the page session: a dismissal outlives the editor
// being closed and reopened, the way a draft does, and a reload of the page
// asks again, since by then the person may have forgotten what they decided.

const dismissedNotices = new Set<string>()

function noticeKey(rootKey: string, path: string): string {
  return `${rootKey}\u0000${path}`
}

export function fallbackNoticeDismissed(rootKey: string, path: string): boolean {
  return dismissedNotices.has(noticeKey(rootKey, path))
}

export function dismissFallbackNotice(rootKey: string, path: string): void {
  dismissedNotices.add(noticeKey(rootKey, path))
}

// For tests only: the set is page-lifetime state.
export function resetFallbackNotices(): void {
  dismissedNotices.clear()
}

// Whether the notice is up for a buffer: the encoding was the fallback rather
// than something the file or the person said, and nobody has waved it off.
export function showsFallbackNotice(
  buffer: { encodingFallback?: boolean } | undefined,
  dismissed: boolean,
): boolean {
  return (buffer?.encodingFallback ?? false) && !dismissed
}
