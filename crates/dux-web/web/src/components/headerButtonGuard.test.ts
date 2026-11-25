// Every button in a pane's header is built from `HeaderButton`, so the Changes
// pane's, the centre pane's and the editor's look, size and space alike. This
// guard reads the source of each header and fails on a button written there by
// hand: a `<Button>`, a native `<button>`, a `buttonVariants(...)` class, or a
// `<ButtonPrimitive>`. It reads text, not the program, so it cannot see:
//
// - a header that is not in `HEADERS` (a new one is found only when it imports
//   `HeaderButton` and is then asked to be listed, or when a reviewer adds it);
// - a header's button drawn by a component that is not in `HEADERS`' regions
//   (a button inside a helper component the header renders), a role-and-class
//   look-alike on a `div`, or an anchor styled as a button;
// - a button chosen at run time (`const Tag = Button`, `createElement(Button)`)
//   or spread in from another module;
// - the phone's own headers, flap and floating pill, which are not part of this
//   piece; and test files, which it skips.
//
// It does read the one way round the shared box: `HEADER_BOX` is used by the
// `⋯` trigger and `HeaderButton` alone.
import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { filesUnder, withoutComments } from "@/test/sourceFiles"

const srcDir = fileURLToPath(new URL("../", import.meta.url))

/** The header code of each file: the functions it is drawn by, found by name, or
 * `"*"` for the whole file. A function runs from its `function` line to the
 * first later line that closes it at the same indent. */
const HEADERS: Record<string, string[] | "*"> = {
  "components/InsetHeader.tsx": "*",
  "components/TheaterToggle.tsx": "*",
  "components/AppMenu.tsx": ["AppMenu"],
  "components/StackNewDialog.tsx": ["NewTabButton"],
  "components/MacroPopover.tsx": ["TriggerButton", "DisabledTrigger"],
  "components/ChangedFiles.tsx": ["ChangesHeader", "OpenEditorButton"],
  "components/EditorBody.tsx": [
    "renderHeader",
    "renderDesktopViewActions",
    "renderSaveAction",
    "renderMobileActions",
    "renderStandaloneActions",
    "renderRootTitle",
  ],
}

/** Each hand-built button that may stand, by file and a text its opening tag
 * holds, used exactly once, with its reason. */
const ALLOWED: Array<{ file: string; contains: string; reason: string }> = [
  {
    file: "components/EditorBody.tsx",
    contains: 'aria-label="Back"',
    reason:
      "the phone's Up control: navigation, so quieter than the acting buttons beside it (ghost), since navigation matches an action cluster on geometry and differs on weight",
  },
  {
    file: "components/MacroPopover.tsx",
    contains: 'aria-label="Run a macro"',
    reason:
      "the macro trigger inside the floating theater pill, a round ghost button in a surface that is not a header",
  },
]

/** The opening tag of each `Button`, `button` or `ButtonPrimitive`, and each
 * `buttonVariants(` call, in `code`, with its attributes to the tag's end. */
export function handBuilt(code: string): string[] {
  const found: string[] = []
  for (const m of code.matchAll(/<(?:Button|button|ButtonPrimitive)(?![\w.])|\bbuttonVariants\s*\(/g)) {
    found.push(tagText(code, m.index))
  }
  return found
}

// From `start` to the `>` that ends the tag (outside braces and strings, and
// not the `>` of an arrow), or a call's closing bracket for a function.
function tagText(code: string, start: number): string {
  const call = code[start] === "b"
  let depth = 0
  let quote: string | null = null
  for (let i = start; i < code.length; i++) {
    const c = code[i]
    if (quote !== null) {
      if (c === "\\") i++
      else if (c === quote) quote = null
      continue
    }
    if (c === '"' || c === "'" || c === "`") quote = c
    else if (c === "{" || c === "(") depth++
    else if (c === "}" || c === ")") {
      depth--
      if (depth === 0 && call) return code.slice(start, i + 1)
    } else if (c === ">" && depth <= 0 && !call && code[i - 1] !== "=") return code.slice(start, i + 1)
  }
  return code.slice(start)
}

/** The source of the function `name`, from its line to the line that closes it. */
export function functionSource(code: string, name: string): string | null {
  const lines = code.split("\n")
  const head = new RegExp(String.raw`^(\s*)(?:export\s+)?function\s+${name}\b`)
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(head)
    if (!m) continue
    for (let j = i + 1; j < lines.length; j++) {
      if (lines[j] === `${m[1]}}`) return lines.slice(i, j + 1).join("\n")
    }
  }
  return null
}

function headerCode(file: string, regions: string[] | "*"): string[] {
  const code = withoutComments(readFileSync(`${srcDir}${file}`, "utf8"))
  if (regions === "*") return [code]
  return regions.map((name) => {
    const body = functionSource(code, name)
    if (body === null) throw new Error(`${file} has no function ${name}: update HEADERS`)
    return body
  })
}

describe("every header button is a HeaderButton (guard)", () => {
  it("finds no hand-built button in a pane header but the listed ones", () => {
    const used = new Map<number, number>()
    const offenders: string[] = []
    for (const [file, regions] of Object.entries(HEADERS)) {
      for (const tag of headerCode(file, regions).flatMap(handBuilt)) {
        const at = ALLOWED.findIndex((a) => a.file === file && tag.includes(a.contains))
        if (at < 0) offenders.push(`${file}: ${tag.replace(/\s+/g, " ").slice(0, 100)}`)
        else used.set(at, (used.get(at) ?? 0) + 1)
      }
    }
    expect(offenders, "build a header's button with HeaderButton").toEqual([])
    // Each exemption stands for one button: a second one of its kind, or none
    // left, means the entry is out of date.
    ALLOWED.forEach((a, i) => expect(used.get(i), `${a.file} ${a.contains}`).toBe(1))
  })

  it("lists every file that builds a button with HeaderButton as a header", () => {
    const unlisted = filesUnder(srcDir)
      .filter((f) => /\.tsx$/.test(f) && !/\.test\./.test(f))
      .map((f) => relative(srcDir, f))
      .filter((f) => f !== "components/HeaderButton.tsx" && f !== "components/headerButtonTypeContract.tsx")
      .filter((f) => /<HeaderButton\b/.test(readFileSync(`${srcDir}${f}`, "utf8")))
      .filter((f) => !(f in HEADERS))
    expect(unlisted, "add the file to HEADERS, with the functions its header is drawn by").toEqual([])
  })

  it("keeps the header's shared box to the ⋯ and HeaderButton", () => {
    const users = filesUnder(srcDir)
      .filter((f) => /\.tsx?$/.test(f) && !/\.test\./.test(f))
      .map((f) => relative(srcDir, f))
      .filter((f) => /\bHEADER_BOX\b/.test(withoutComments(readFileSync(`${srcDir}${f}`, "utf8"))))
      .sort()
    expect(users).toEqual([
      "components/EllipsisMenuTrigger.tsx",
      "components/HeaderButton.tsx",
      "components/ellipsisPlacements.ts",
    ])
  })

  it("sees each hand-built button that reading text can see", () => {
    const sees = [
      '<Button variant="outline">x</Button>',
      "<Button\n  variant=\"outline\"\n  onClick={() => go()}\n>",
      "<button type=\"button\" className=\"x\">",
      "<ButtonPrimitive>",
      'className={buttonVariants({ variant: "outline" })}',
      "<Button>",
    ]
    for (const src of sees) expect(handBuilt(src), src).toHaveLength(1)
    const passes = [
      '<HeaderButton icon={<X />} label="Close" />',
      "<ButtonGroup>",
      '<EllipsisMenuTrigger placement="header" />',
      "// <Button> in a comment is stripped before it is read",
    ]
    for (const src of passes) expect(handBuilt(withoutComments(src)), src).toHaveLength(0)
  })

  it("keeps a function's end by its indent", () => {
    const code = "  function a() {\n    return 1\n  }\n  function b() {\n    <Button>\n  }\n"
    expect(functionSource(code, "b")).toContain("<Button>")
    expect(functionSource(code, "a")).not.toContain("<Button>")
    expect(functionSource(code, "c")).toBeNull()
  })
})
