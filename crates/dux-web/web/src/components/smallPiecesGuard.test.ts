import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { classGroupsOf } from "@/test/classGroups"
import { sourceFilesUnder, withoutComments } from "@/test/sourceFiles"

// Three small pieces are each one component: the unsaved-edits mark is
// `UnsavedDot`, an icon in a rounded square is `IconTile`, and a pill label is
// `Badge`. This guard reads every source file under `src` (tests excluded) and
// fails on a copy of one built by hand:
//
// - the unsaved-edits dot: the `●` glyph, written or escaped, in code;
// - an icon tile: one class group with a numeric square size (`size-N`), a
//   `rounded-md`, `rounded-lg` or `rounded-xl` corner, a solid `bg-muted` and
//   its content centred (`items-center` and `justify-center`);
// - a pill label: one class group with a `rounded-full` or `rounded-4xl`
//   corner, horizontal padding, small text (`text-xs`, `text-[10px]`,
//   `text-[11px]`) and a background fill, and no size or tall height that
//   would make it a control.
//
// Each allowed occurrence is matched by its file and its text, one entry to one
// occurrence. It reads text, not the program: classes split across variables
// (as `IconTile` and `Badge` themselves build theirs), built at runtime or set
// in a stylesheet are not seen, nor is a dot drawn with CSS.

const root = fileURLToPath(new URL("../", import.meta.url))

// The components that own the pieces.
const HOME = new Set(["components/UnsavedDot.tsx", "components/IconTile.tsx", "components/ui/badge.tsx"])

const classesOf = (group: string) => group.split(" ")
const hasClass = (group: string, re: RegExp) => classesOf(group).some((c) => re.test(c))

function isIconTile(group: string): boolean {
  return (
    hasClass(group, /^size-\d+(?:\.\d+)?$/) &&
    hasClass(group, /^rounded-(?:md|lg|xl)$/) &&
    hasClass(group, /^bg-muted$/) &&
    hasClass(group, /^items-center$/) &&
    hasClass(group, /^justify-center$/)
  )
}

function isPillLabel(group: string): boolean {
  return (
    hasClass(group, /^rounded-(?:full|4xl)$/) &&
    hasClass(group, /^px-/) &&
    hasClass(group, /^text-(?:xs|\[1[01]px\])$/) &&
    hasClass(group, /^bg-/) &&
    !hasClass(group, /^(?:size-|min-h-|h-(?:[6-9]|1\d)\b)/)
  )
}

const DOT = /●|\\u25cf|\\u\{25cf\}/i

interface Found {
  file: string
  what: string
  text: string
}

function findSmallPieces(files: Record<string, string>): Found[] {
  const out: Found[] = []
  for (const [file, source] of Object.entries(files)) {
    if (HOME.has(file)) continue
    for (const line of withoutComments(source).split("\n")) {
      if (DOT.test(line)) out.push({ file, what: "an unsaved-edits dot", text: line.trim() })
    }
    for (const group of classGroupsOf(source)) {
      if (isIconTile(group)) out.push({ file, what: "an icon tile", text: group })
      else if (isPillLabel(group)) out.push({ file, what: "a pill label", text: group })
    }
  }
  return out
}

interface Allowed {
  file: string
  text: string
  reason: string
}
const ALLOWED: Allowed[] = [
  {
    file: "lib/tabTitle.ts",
    text: 'const DIRTY_MARK = "● "',
    reason: "the browser tab's title, which is text and cannot hold a component",
  },
  {
    file: "components/ui/empty.tsx",
    text: "mb-2 flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted",
    reason: "the empty-state primitive's own icon slot, which `EmptyState` draws",
  },
]

// Match each occurrence to one entry; what is left over on either side is wrong.
function unmatched(found: Found[], allowed: Allowed[]) {
  const free = [...allowed]
  const unexplained: string[] = []
  for (const hit of found) {
    const at = free.findIndex((a) => a.file === hit.file && hit.text.includes(a.text))
    if (at === -1) unexplained.push(`${hit.file}: ${hit.what}: ${hit.text}`)
    else free.splice(at, 1)
  }
  return { unexplained, stale: free.map((a) => `${a.file}: ${a.text}`) }
}

describe("guard: one unsaved dot, one icon tile, one pill label", () => {
  it("draws each through UnsavedDot, IconTile or Badge unless that occurrence is allowed", () => {
    const files = Object.fromEntries(
      sourceFilesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
    )
    const { unexplained, stale } = unmatched(findSmallPieces(files), ALLOWED)
    expect(unexplained, "use UnsavedDot, IconTile or Badge, or allow this occurrence with its reason").toEqual([])
    expect(stale, "an allowed occurrence is gone: remove its entry").toEqual([])
  })

  it("sees each hand-built piece and ignores look-alikes", () => {
    const seen = (source: string) => findSmallPieces({ "components/X.tsx": source }).map((f) => f.what)
    expect(seen("<span>●</span>")).toEqual(["an unsaved-edits dot"])
    expect(seen('const d = "\\u25CF"')).toEqual(["an unsaved-edits dot"])
    expect(seen("// ● in a comment")).toEqual([])
    expect(
      seen('<span className="flex size-9 items-center justify-center rounded-lg bg-muted" />'),
    ).toEqual(["an icon tile"])
    expect(
      seen('<span className="flex size-9 items-center justify-center rounded-full bg-muted" />'),
    ).toEqual([])
    expect(
      seen('<span className="flex size-9 items-center justify-center rounded-lg bg-muted/40" />'),
    ).toEqual([])
    expect(seen('<span className="rounded-full bg-muted px-2 text-xs">3</span>')).toEqual([
      "a pill label",
    ])
    expect(seen('<span className={cn("rounded-4xl px-1.5", "text-[11px] bg-secondary")} />')).toEqual([
      "a pill label",
    ])
    expect(seen('<button className="h-9 rounded-full bg-muted px-3 text-xs" />')).toEqual([])
    expect(seen('<span className="rounded-full border px-2 text-xs" />')).toEqual([])
    expect(findSmallPieces({ "components/UnsavedDot.tsx": "<span>●</span>" })).toEqual([])
  })

  it("holds one allowed entry to one occurrence", () => {
    const entry = { file: "components/X.tsx", text: "●", reason: "x" }
    const hit = { file: "components/X.tsx", what: "an unsaved-edits dot", text: "<b>●</b>" }
    expect(unmatched([hit, hit], [entry]).unexplained).toHaveLength(1)
    expect(unmatched([hit], [entry, entry]).stale).toHaveLength(1)
  })
})
