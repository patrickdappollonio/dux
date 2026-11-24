import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { sourceFilesUnder, withoutComments } from "@/test/sourceFiles"

// Why a menu item is disabled is said by the shared `MenuReason` pieces, which
// own where the sentence sits, its width and type, and the id the items name.
// This guard reads every source file under `src` (tests excluded), comments
// dropped, and fails on the three marks a hand-built reason leaves:
//
// - a menu label given anything (`<DropdownMenuLabel id=…` or `className=…`,
//   the same for `ContextMenuLabel`): a group heading is written bare, and a
//   label with an id or a width was a reason;
// - a menu group label reached directly (`GroupLabel`) outside the menu
//   primitives and `MenuReason.tsx`;
// - a menu item, submenu trigger, checkbox or radio item whose
//   `aria-describedby` does not name a reason's `describedBy`.
//
// It reads text, not the program. It cannot see a description passed in a
// spread (`{...props}`), a reason drawn as a plain `<div>` or `<p>` that no
// item names, or an identifier called `describedBy` that holds something else.
// A comment start in an unusual place (a regular expression holding a quote or
// `//`) can hide the rest of its line.

const root = fileURLToPath(new URL("../", import.meta.url))

type Mark = "label-with-props" | "group-label" | "hand-wired-description"

interface Finding {
  file: string
  mark: Mark
  text: string
}

const LABEL_WITH_PROPS = /<(?:DropdownMenu|ContextMenu)Label\s+[\w{]/g
const GROUP_LABEL = /\bGroupLabel\b/g
const ITEM_TAG = /<(?:DropdownMenu|ContextMenu)(?:Item|SubTrigger|CheckboxItem|RadioItem)\b/g

// The opening tag that starts at `start`, up to its closing `>` outside braces
// and quotes, so a `=>` inside an attribute does not end it.
function openingTag(source: string, start: number): string {
  let depth = 0
  let quote: string | null = null
  for (let i = start + 1; i < source.length; i++) {
    const c = source[i]
    if (quote !== null) {
      if (c === quote) quote = null
      continue
    }
    if (c === '"' || c === "'" || c === "`") quote = c
    else if (c === "{") depth++
    else if (c === "}") depth--
    else if (c === ">" && depth === 0) return source.slice(start, i + 1)
  }
  return source.slice(start)
}

// The braced expression after `aria-describedby=` in a tag, or the quoted value.
function describedByValues(tag: string): string[] {
  const out: string[] = []
  const attr = /aria-describedby=/g
  let m: RegExpExecArray | null
  while ((m = attr.exec(tag)) !== null) {
    const at = m.index + m[0].length
    if (tag[at] !== "{") {
      out.push(tag.slice(at).split(/\s/)[0] ?? "")
      continue
    }
    let depth = 0
    for (let i = at; i < tag.length; i++) {
      if (tag[i] === "{") depth++
      else if (tag[i] === "}" && --depth === 0) {
        out.push(tag.slice(at, i + 1))
        break
      }
    }
  }
  return out
}

export function findHandBuiltReasons(files: Record<string, string>): Finding[] {
  const out: Finding[] = []
  for (const [file, raw] of Object.entries(files)) {
    const source = withoutComments(raw)
    const lineAt = (i: number) => source.slice(source.lastIndexOf("\n", i) + 1, source.indexOf("\n", i)).trim()
    for (const m of source.matchAll(LABEL_WITH_PROPS)) {
      out.push({ file, mark: "label-with-props", text: lineAt(m.index) })
    }
    for (const m of source.matchAll(GROUP_LABEL)) {
      out.push({ file, mark: "group-label", text: lineAt(m.index) })
    }
    for (const m of source.matchAll(ITEM_TAG)) {
      for (const value of describedByValues(openingTag(source, m.index))) {
        if (!/describedBy\b/.test(value)) {
          out.push({ file, mark: "hand-wired-description", text: value.replace(/\s+/g, " ") })
        }
      }
    }
  }
  return out
}

// One entry per exact occurrence. An entry that matches nothing fails the suite.
const ALLOWED: { file: string; mark: Mark; contains: string; reason: string }[] = [
  { file: "components/MenuReason.tsx", mark: "group-label", contains: "<MenuPrimitive.GroupLabel", reason: "the shared reason line itself" },
  { file: "components/MenuReason.tsx", mark: "group-label", contains: "</MenuPrimitive.GroupLabel>", reason: "its closing tag" },
  { file: "components/ui/dropdown-menu.tsx", mark: "group-label", contains: "MenuPrimitive.GroupLabel.Props", reason: "the primitive's heading part" },
  { file: "components/ui/dropdown-menu.tsx", mark: "group-label", contains: "<MenuPrimitive.GroupLabel", reason: "the primitive's heading part" },
  { file: "components/ui/context-menu.tsx", mark: "group-label", contains: "ContextMenuPrimitive.GroupLabel.Props", reason: "the primitive's heading part" },
  { file: "components/ui/context-menu.tsx", mark: "group-label", contains: "<ContextMenuPrimitive.GroupLabel", reason: "the primitive's heading part" },
  { file: "components/ui/select.tsx", mark: "group-label", contains: "SelectPrimitive.GroupLabel.Props", reason: "a select's group heading, not a menu" },
  { file: "components/ui/select.tsx", mark: "group-label", contains: "<SelectPrimitive.GroupLabel", reason: "a select's group heading, not a menu" },
]

function unallowed(findings: Finding[]): { rest: Finding[]; unused: string[] } {
  const used = new Set<number>()
  const rest = findings.filter((f) => {
    const i = ALLOWED.findIndex(
      (a, k) => !used.has(k) && a.file === f.file && a.mark === f.mark && f.text.includes(a.contains),
    )
    if (i < 0) return true
    used.add(i)
    return false
  })
  const unused = ALLOWED.filter((_, i) => !used.has(i)).map((a) => `${a.file}: ${a.contains}`)
  return { rest, unused }
}

describe("a disabled menu item's reason is the shared MenuReason", () => {
  it("finds no hand-built one in any folder under src", () => {
    const files = Object.fromEntries(
      sourceFilesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
    )
    const { rest, unused } = unallowed(findHandBuiltReasons(files))
    expect(
      rest.map((f) => `${f.file} (${f.mark}): ${f.text}`),
      "say it with useMenuReason and MenuReasonsHead or MenuItemReason, or allow the exact occurrence with a reason",
    ).toEqual([])
    expect(unused, "an allowed occurrence that no longer exists: remove its entry").toEqual([])
  })

  it("sees each mark, wherever it sits", () => {
    const cases: Record<string, string> = {
      "a/one.tsx": `<DropdownMenuLabel id={reasonId} className="max-w-64">{why}</DropdownMenuLabel>`,
      "a/b/two.tsx": `<ContextMenuLabel\n  className="max-w-60 whitespace-normal"\n>{why}</ContextMenuLabel>`,
      "three.tsx": `<Menu.GroupLabel id={id}>{why}</Menu.GroupLabel>`,
      "four.tsx": `<DropdownMenuItem\n  onClick={() => go()}\n  aria-describedby={blocked ? reasonId : undefined}\n>Go</DropdownMenuItem>`,
      "five.tsx": `<DropdownMenuSubTrigger aria-describedby="why">More</DropdownMenuSubTrigger>`,
    }
    const hit = new Set(findHandBuiltReasons(cases).map((f) => f.file))
    expect([...hit].sort()).toEqual(Object.keys(cases).sort())
  })

  it("passes a heading and a described item", () => {
    const fine = {
      "ok.tsx": `<DropdownMenuLabel>{MENU_GROUP_LABELS.running}</DropdownMenuLabel>\n<DropdownMenuItem disabled={x} aria-describedby={reason.describedBy} onClick={() => go()}>Go</DropdownMenuItem>`,
    }
    expect(findHandBuiltReasons(fine)).toEqual([])
  })
})
