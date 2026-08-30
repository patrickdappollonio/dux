// A deliberately small YAML front-matter reader for the editor's markdown
// preview, which renders the leading `--- … ---` block as a key/value table the
// way GitHub does.
//
// This is NOT a YAML implementation and is not trying to become one: dux ships
// no YAML parser and pulling one in for a preview table would be a large
// dependency for a decorative feature. It reads the subset front matter is
// actually written in (scalars, quoted strings, flow and block lists, one level
// of nested map) and falls back to showing the raw text for anything else. It
// never throws and never drops text: a value it cannot read is displayed
// verbatim, and a block it produces no rows at all for is reported through
// `unreadable` so the caller can show the block itself. The block is stripped
// from the body either way, so silence there would lose what the author wrote.
//
// Known limits, stated rather than hidden: anchors, aliases, tags, multi-document
// streams and maps nested more than one level deep are shown as raw text; flow
// maps (`{a: 1}`) are raw text; comments are stripped only when they follow a
// space on an unquoted scalar, which is the form YAML itself requires.

export type FrontMatterScalar = string | number | boolean | null
export type FrontMatterValue = FrontMatterScalar | FrontMatterScalar[]

export interface FrontMatterRow {
  // Display key. A nested map contributes one row per leaf, keyed `parent.child`.
  key: string
  value: FrontMatterValue
}

export interface FrontMatterSplit {
  rows: FrontMatterRow[]
  // The block's own text, when it holds content this reader produced no rows
  // for. The caller shows it verbatim: the block is stripped from the body, so
  // dropping it silently would lose text the author wrote.
  unreadable: string | null
  // The document with the front-matter block removed, ready for the markdown
  // renderer.
  body: string
}

// Split a document into its front-matter rows and its markdown body, or null
// when there is no front matter. Front matter exists only when the document's
// FIRST line is exactly `---` and a later line is exactly `---` or `...`.
export function splitFrontMatter(content: string): FrontMatterSplit | null {
  const lines = stripByteOrderMark(content).split("\n").map(stripCarriageReturn)
  if (lines.length === 0) return null
  if (lines[0].trimEnd() !== "---") return null
  let end = -1
  for (let i = 1; i < lines.length; i += 1) {
    const line = lines[i].trimEnd()
    if (line === "---" || line === "...") {
      end = i
      break
    }
  }
  if (end === -1) return null
  const block = lines.slice(1, end)
  const rows = parseFrontMatterBlock(block)
  const kept = trimBlankEdges(block)
  const hasContent = kept.some((line) => !isIgnorable(line))
  return {
    rows,
    unreadable: rows.length === 0 && hasContent ? kept.join("\n") : null,
    body: lines.slice(end + 1).join("\n"),
  }
}

// Parse the lines BETWEEN the fences. Exported for tests and so a caller with
// its own extraction can reuse the value reader.
export function parseFrontMatterBlock(lines: string[]): FrontMatterRow[] {
  const rows: FrontMatterRow[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (isIgnorable(line) || indentOf(line) > 0) {
      i += 1
      continue
    }
    const entry = splitKey(line)
    if (entry === null) {
      i += 1
      continue
    }
    const block: string[] = []
    let j = i + 1
    while (j < lines.length && (isBlank(lines[j]) || indentOf(lines[j]) > 0)) {
      block.push(lines[j])
      j += 1
    }
    pushEntry(rows, entry.key, entry.rest, trimBlankEdges(block))
    i = j
  }
  return rows
}

// Render one value the way the table shows it. Lists join inline with commas,
// which is what GitHub does with front-matter arrays.
export function formatFrontMatterValue(value: FrontMatterValue): string {
  if (Array.isArray(value)) return value.map(formatScalar).join(", ")
  return formatScalar(value)
}

function formatScalar(value: FrontMatterScalar): string {
  // An absent value is an empty cell. The word "null" in a table reads as a
  // value the author wrote, and usually they wrote nothing at all.
  if (value === null) return ""
  if (typeof value === "boolean") return value ? "true" : "false"
  if (typeof value === "number") return String(value)
  return value
}

function pushEntry(
  rows: FrontMatterRow[],
  key: string,
  rest: string,
  block: string[],
): void {
  if (rest === "|" || rest === ">" || /^[|>][-+\d]*$/.test(rest)) {
    rows.push({ key, value: foldBlockScalar(rest, block) })
    return
  }
  if (rest !== "") {
    rows.push({ key, value: parseScalarOrList(rest) })
    return
  }
  if (block.length === 0) {
    rows.push({ key, value: null })
    return
  }
  const first = block.find((line) => !isIgnorable(line))
  if (first === undefined) {
    rows.push({ key, value: null })
    return
  }
  if (isListItem(first)) {
    const items = readBlockList(block)
    rows.push({ key, value: items ?? rawText(block) })
    return
  }
  const nested = readNestedMap(key, block)
  if (nested.length > 0) {
    rows.push(...nested)
    return
  }
  rows.push({ key, value: rawText(block) })
}

// One level of nesting becomes `parent.child` rows. A child that itself opens a
// deeper block keeps that block as raw text rather than growing a third column.
function readNestedMap(parent: string, block: string[]): FrontMatterRow[] {
  const base = indentOf(block.find((line) => !isIgnorable(line)) ?? "")
  const rows: FrontMatterRow[] = []
  let i = 0
  while (i < block.length) {
    const line = block[i]
    if (isIgnorable(line)) {
      i += 1
      continue
    }
    // Deeper lines are consumed as a child's own block below, so anything left
    // at a different indent is a mis-indented sibling: bail rather than drop it.
    if (indentOf(line) !== base) return []
    const entry = splitKey(line)
    // A line that is neither an entry at this level nor a continuation of one
    // means this block is not the shape we read. Give up on the whole block so
    // the caller shows its text rather than a table missing rows.
    if (entry === null) return []
    const inner: string[] = []
    let j = i + 1
    while (j < block.length && (isBlank(block[j]) || indentOf(block[j]) > base)) {
      inner.push(block[j])
      j += 1
    }
    const trimmed = trimBlankEdges(inner)
    const key = `${parent}.${entry.key}`
    if (entry.rest !== "") {
      rows.push({ key, value: parseScalarOrList(entry.rest) })
    } else if (trimmed.length === 0) {
      rows.push({ key, value: null })
    } else if (isListItem(trimmed.find((l) => !isIgnorable(l)) ?? "")) {
      const items = readBlockList(trimmed)
      if (items === null) return []
      rows.push({ key, value: items })
    } else {
      rows.push({ key, value: rawText(trimmed) })
    }
    i = j
  }
  return rows
}

// A block list of plain scalars, or null when it is something else: a list of
// maps whose continuation lines this would drop, or a `-b` that is not an item
// at all. Null means the caller shows the block's raw text.
function readBlockList(block: string[]): FrontMatterScalar[] | null {
  const items: FrontMatterScalar[] = []
  for (const line of block) {
    if (isIgnorable(line)) continue
    if (!isListItem(line)) return null
    items.push(parseScalar(line.trimStart().replace(/^-\s*/, "")))
  }
  return items
}

function foldBlockScalar(marker: string, block: string[]): string {
  const base = indentOf(block.find((line) => !isBlank(line)) ?? "")
  const body = block.map((line) => line.slice(base))
  const joiner = marker.startsWith(">") ? " " : "\n"
  return body.join(joiner).trim()
}

function rawText(block: string[]): string {
  return block
    .filter((line) => !isBlank(line))
    .map((line) => line.trim())
    .join(" ")
}

function parseScalarOrList(text: string): FrontMatterValue {
  const flow = parseFlowList(text)
  return flow === null ? parseScalar(text) : flow
}

// `[a, b, "c, d"]`. Returns null when the text is not a well-formed flow list,
// so the caller falls back to showing it as a plain string.
function parseFlowList(text: string): FrontMatterScalar[] | null {
  if (!text.startsWith("[") || !text.endsWith("]")) return null
  const inner = text.slice(1, -1).trim()
  if (inner === "") return []
  const items: string[] = []
  let current = ""
  let quote: string | null = null
  for (const ch of inner) {
    if (quote !== null) {
      current += ch
      if (ch === quote) quote = null
      continue
    }
    if (ch === '"' || ch === "'") {
      quote = ch
      current += ch
      continue
    }
    if (ch === "[" || ch === "]" || ch === "{" || ch === "}") return null
    if (ch === ",") {
      items.push(current)
      current = ""
      continue
    }
    current += ch
  }
  if (quote !== null) return null
  items.push(current)
  return items.map((item) => parseScalar(item.trim()))
}

// A single YAML scalar. Anything this does not recognize is its own text, which
// is the fallback that keeps the table honest instead of empty.
export function parseScalar(text: string): FrontMatterScalar {
  const trimmed = text.trim()
  const quoted = unquote(trimmed)
  if (quoted !== null) return quoted
  const bare = stripTrailingComment(trimmed)
  // A quoted scalar may carry a trailing comment, which leaves the quotes on the
  // end of `trimmed`: retry the unquote once the comment is off.
  const quotedBeforeComment = bare === trimmed ? null : unquote(bare)
  if (quotedBeforeComment !== null) return quotedBeforeComment
  if (bare === "" || bare === "~") return null
  if (/^null$/i.test(bare)) return null
  if (/^(true|yes|on)$/i.test(bare)) return true
  if (/^(false|no|off)$/i.test(bare)) return false
  if (/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(bare)) {
    const n = Number(bare)
    // Only when the number survives the round trip. `007`, `1.10`, `-0` and
    // integers past the float range all come back different, and a table must
    // show what the author wrote, not what JavaScript made of it.
    if (Number.isFinite(n) && String(n) === bare) return n
  }
  return bare
}

// A fully quoted scalar, unescaped. Returns null when the text is not one, so a
// value with a stray quote in it stays raw rather than being mangled.
function unquote(text: string): string | null {
  if (text.length < 2) return null
  const q = text[0]
  if (q !== '"' && q !== "'") return null
  if (!text.endsWith(q)) return null
  const inner = text.slice(1, -1)
  if (q === "'") {
    if (/(^|[^'])'($|[^'])/.test(inner)) return null
    return inner.replace(/''/g, "'")
  }
  // A double-quoted scalar may contain escaped quotes; an unescaped one means
  // the text is not a single quoted scalar.
  let out = ""
  for (let i = 0; i < inner.length; i += 1) {
    const ch = inner[i]
    if (ch === "\\" && i + 1 < inner.length) {
      const next = inner[i + 1]
      out += next === "n" ? "\n" : next === "t" ? "\t" : next
      i += 1
      continue
    }
    if (ch === '"') return null
    out += ch
  }
  return out
}

// YAML ends an unquoted scalar at ` #`. The leading space matters: it is what
// keeps `http://host/page#anchor` intact.
function stripTrailingComment(text: string): string {
  const at = text.search(/\s#/)
  return at === -1 ? text : text.slice(0, at).trimEnd()
}

function splitKey(line: string): { key: string; rest: string } | null {
  const text = line.trim()
  let quote: string | null = null
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i]
    if (quote !== null) {
      if (ch === quote) quote = null
      continue
    }
    if (ch === '"' || ch === "'") {
      quote = ch
      continue
    }
    if (ch === ":" && (i + 1 === text.length || /\s/.test(text[i + 1]))) {
      const rawKey = text.slice(0, i).trim()
      if (rawKey === "") return null
      return {
        key: unquote(rawKey) ?? rawKey,
        rest: text.slice(i + 1).trim(),
      }
    }
  }
  return null
}

function isListItem(line: string): boolean {
  const text = line.trimStart()
  return text === "-" || text.startsWith("- ")
}

function isBlank(line: string): boolean {
  return line.trim() === ""
}

function isIgnorable(line: string): boolean {
  return isBlank(line) || line.trimStart().startsWith("#")
}

function indentOf(line: string): number {
  return line.length - line.trimStart().length
}

function trimBlankEdges(lines: string[]): string[] {
  let start = 0
  let end = lines.length
  while (start < end && isBlank(lines[start])) start += 1
  while (end > start && isBlank(lines[end - 1])) end -= 1
  return lines.slice(start, end)
}

// An editor that writes a BOM puts it before the opening fence, where it would
// otherwise make the first line something other than `---`.
function stripByteOrderMark(content: string): string {
  return content.startsWith("﻿") ? content.slice(1) : content
}

function stripCarriageReturn(line: string): string {
  return line.endsWith("\r") ? line.slice(0, -1) : line
}
