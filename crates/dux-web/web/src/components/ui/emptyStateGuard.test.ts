// A surface with nothing to show says so through the shared `EmptyState`, which
// decides its padding, type and colour. This guard reads the source of every
// component and fails on the look a hand-built one takes: a class string that
// sets small muted text and a roomy block around it (vertical padding of a
// line or more, a fixed 16rem box, or auto margins).
//
// It reads class strings, not the program, so it cannot see one built from a
// variable, a conditional part or `cn` pieces split across strings, nor one
// written with a tighter padding than it looks for.
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const srcDir = fileURLToPath(new URL("../../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

const ROOMY = /^(?:py-(?:4|5|6|8|10|12|16)|p-(?:4|5|6|8)|h-64|m-auto)$/

function looksLikeEmptyState(classes: string): boolean {
  const parts = classes.split(/\s+/)
  return (
    parts.includes("text-sm") &&
    parts.includes("text-muted-foreground") &&
    parts.some((part) => ROOMY.test(part))
  )
}

// Lines with the same look that are not an empty state, each with its reason.
// An entry that no longer matches anything fails the suite, so the list cannot
// rot.
const ALLOWED: { file: string; classes: string; reason: string }[] = [
  {
    file: "components/PullRequestsDialog.tsx",
    classes: "py-6 text-center text-sm text-muted-foreground",
    reason: "The branches tab while it reads them, or why it could not: a loading or error line.",
  },
  {
    file: "components/MacrosDialog.tsx",
    classes: "py-10 text-center text-sm text-muted-foreground",
    reason: "The macros while they load: a loading line.",
  },
  {
    file: "components/ConfigEditorDialog.tsx",
    classes: "py-12 text-center text-sm text-muted-foreground",
    reason: "config.toml while it loads: a loading line.",
  },
  {
    file: "components/ChangeBaseBranchDialog.tsx",
    classes: "flex items-center gap-2 px-2 py-4 text-sm text-muted-foreground",
    reason: "The branches while they load, beside a spinner: a loading line.",
  },
]

function hits(): { file: string; classes: string }[] {
  return walk(join(srcDir, "components"))
    .map((path) => relative(srcDir, path))
    .filter((name) => /\.tsx?$/.test(name) && !name.includes(".test."))
    .filter((name) => name !== "components/ui/empty.tsx")
    .flatMap((name) => {
      const source = readFileSync(join(srcDir, name), "utf-8")
      return [...source.matchAll(/(["`])([^"`\n]*)\1/g)]
        .map((m) => m[2])
        .filter(looksLikeEmptyState)
        .map((classes) => ({ file: name, classes }))
    })
}

describe("empty states go through the shared EmptyState", () => {
  it("finds no hand-built empty state", () => {
    const found = hits().filter(
      (hit) => !ALLOWED.some((a) => a.file === hit.file && a.classes === hit.classes),
    )
    expect(found, "use EmptyState from components/ui/empty").toEqual([])
  })

  it("lists only exceptions that still exist", () => {
    const all = hits()
    const stale = ALLOWED.filter(
      (a) => !all.some((hit) => hit.file === a.file && hit.classes === a.classes),
    )
    expect(stale).toEqual([])
  })
})
