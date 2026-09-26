// Every variable a modal shows (a branch, a path, a file, a command, an agent,
// project or terminal name, a pull request number) renders through the one
// `InlineCode` chip, and the chip replaces the quotes that used to delimit it.
// This guard reads the source of every dialog and of the copy builders dialogs
// render, and fails on the two shapes the drift takes: a quote opened right
// before an interpolation, and a hand-rolled monospace span standing in for the
// chip. Toasts and the terminal UI are out of scope.
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const srcDir = fileURLToPath(new URL("../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

// A file that builds prose imports the prose helpers; that is how a copy
// builder is recognised, so a new one is scanned the day it is written.
const PROSE_IMPORT = /from\s+["'](?:\.\/prose|@\/lib\/prose)["']/

// A file that renders a modal imports one of the two dialog primitives, which
// finds a modal whose file is not named like one.
const DIALOG_IMPORT = /from\s+["']@\/components\/ui\/(?:alert-)?dialog["']/

// Which files the guard reads, by rule rather than by list: every dialog under
// components/ (subdirectories included), found by its name or by the dialog
// primitive it imports, and every file under components/ or lib/ that builds
// prose. The prose helpers themselves are the definition of a chip, not a user
// of one.
function isScanned(name: string, source: string): boolean {
  if (!/\.tsx?$/.test(name) || name.includes(".test.")) return false
  if (name === "lib/prose.tsx") return false
  const isDialog =
    (name.startsWith("components/") && /Dialog[^/]*\.tsx$/.test(name)) ||
    DIALOG_IMPORT.test(source)
  return isDialog || PROSE_IMPORT.test(source)
}

function scannedFiles(): { name: string; source: string }[] {
  return [...walk(join(srcDir, "components")), ...walk(join(srcDir, "lib"))]
    .map((path) => ({
      name: relative(srcDir, path),
      source: readFileSync(path, "utf-8"),
    }))
    .filter(({ name, source }) => isScanned(name, source))
}

// A quote that opens immediately before a JSX expression or element, or before
// a template interpolation, is a quote delimiting a name: curly or straight,
// double or single, typed or as an entity. A quote around fixed UI words
// ("Reload config") is not, and is not matched.
const QUOTE_BEFORE_NAME =
  /(&ldquo;|“|&lsquo;|‘|&quot;|&#34;|&#39;|&apos;)[ \t]*[{<]|["'][{<]|["']\$\{/g
// The ad hoc chip the shared component replaced: any monospace span.
const HAND_ROLLED_CHIP = /<span\s+className="[^"]*\bfont-mono\b[^"]*"/g

function nameDelimiterViolations(source: string): string[] {
  return [
    ...[...source.matchAll(QUOTE_BEFORE_NAME)],
    ...[...source.matchAll(HAND_ROLLED_CHIP)],
  ].map((m) => lineAround(source, m.index ?? 0))
}

function lineAround(source: string, index: number): string {
  const start = source.lastIndexOf("\n", index) + 1
  const end = source.indexOf("\n", index)
  return source.slice(start, end === -1 ? undefined : end).trim()
}

// Deliberate exceptions, each with the reason it cannot take the chip. An entry
// that no longer matches anything fails the suite, so the list cannot rot.
const ALLOWED: { file: string; line: string; reason: string }[] = [
  {
    file: "components/StandaloneAgentDialog.tsx",
    line: 'placeholder={`Agent name (optional, defaults to "${standaloneAgentDefaultName(selected)}")`}',
    reason:
      "A placeholder is an attribute and can hold no markup, so the quotes are the only delimiter available.",
  },
  {
    file: "components/FirstLoadDialog.tsx",
    line: '<span className="font-mono">{state.notes?.version ?? ""}</span>',
    reason: "A release number in the heading's badge: a version, not a name in a sentence.",
  },
  {
    file: "components/ProjectList.tsx",
    line: '<span className="shrink-0 font-mono text-xs text-muted-foreground">',
    reason: "The trailing count of a project row (New agent picker, Projects list), and a row is out of the chip rule's scope.",
  },
  {
    file: "components/ProjectsDialog.tsx",
    line: '<span className="min-w-0 truncate font-mono">{project.path}</span>',
    reason: "A project row's folder on its second line: part of the row, not a name in a sentence.",
  },
  {
    file: "components/ChangeBaseBranchDialog.tsx",
    line: '<span className="min-w-0 flex-1 truncate font-mono text-sm">',
    reason: "A branch row that is only its branch: a list row, out of the chip rule's scope.",
  },
  {
    file: "components/TaskManagerDialog.tsx",
    line: '<span className="whitespace-nowrap font-mono text-xs text-muted-foreground">',
    reason: "A process row's command detail: a table cell, out of the chip rule's scope.",
  },
  {
    file: "components/TaskManagerDialog.tsx",
    line: '<span className="whitespace-nowrap font-mono">',
    reason: "A child process's name in a table cell, out of the chip rule's scope.",
  },
  {
    file: "components/TaskManagerDialog.tsx",
    line: '<span className="truncate font-mono">',
    reason: "A child process's name in the phone layout's row, out of the chip rule's scope.",
  },
  {
    file: "components/WorktreesDialog.tsx",
    line: '<span className="font-mono">{entry.branch_name}</span>',
    reason: "The tooltip that recovers a truncated worktree row: part of the row, not a sentence.",
  },
  {
    file: "components/WorktreesDialog.tsx",
    line: '<span className="truncate font-mono text-sm">{entry.branch_name}</span>',
    reason: "A worktree row that is only its branch: a list row, out of the chip rule's scope.",
  },
  // The files below are scanned because they build toast prose, not because
  // they are dialogs; each line is something other than a name in a sentence.
  {
    file: "components/EditorBody.tsx",
    line: '<span className="min-w-0 flex-1 truncate text-left font-mono text-sm [direction:rtl]">',
    reason:
      "The editor header's open path and a search result row: a path that is the whole element, not a name in a sentence.",
  },
  {
    file: "components/EditorBody.tsx",
    line: '<span className="max-w-full shrink-0 truncate font-mono text-xs text-muted-foreground">',
    reason: "The caption under a previewed image: only the file's path, not a sentence.",
  },
  {
    file: "lib/favicon.ts",
    line: '`<svg xmlns="http://www.w3.org/2000/svg" viewBox="${DUCK_VIEWBOX}">` +',
    reason: "SVG markup for the favicon: an attribute value, not prose.",
  },
  {
    file: "lib/favicon.ts",
    line: '`<path fill="${fill}" fill-rule="evenodd" d="${DUCK_PATH}"/>` +',
    reason: "SVG markup for the favicon: attribute values, not prose.",
  },
  {
    file: "lib/fileDrop.ts",
    line: 'return `"${path.replaceAll(/[\\\\"$`]/g, (c) => `\\\\${c}`)}"`',
    reason: "Shell quoting of a pasted path: the quotes are the text the terminal receives.",
  },
  {
    file: "lib/notify.ts",
    line: 'return `Still waiting on the server for "${message}" after ${seconds} seconds. The request has not been answered yet; nothing has been lost, and the outcome will replace this as soon as it arrives.`',
    reason: "Quotes the stranded spinner's own sentence, which is relayed text rather than a name.",
  },
  {
    file: "lib/notify.ts",
    line: 'return `No word from dux about "${message}" for ${seconds} seconds. The operation may still be running, and the connection may simply have dropped. Check dux.log if it never reports back.`',
    reason: "Quotes the stranded spinner's own sentence, which is relayed text rather than a name.",
  },
]

describe("the name-delimiter detector", () => {
  it("flags every shape of a quoted name and a hand-rolled chip", () => {
    for (const bad of [
      "This removes &ldquo;{name}&rdquo; from dux.",
      "No projects match “{query}”.",
      'A branch named “<span className="break-all">{name}</span>”',
      "`dux will ask \"${label}\" to shut down`",
      '<span className="font-mono break-all">{path}</span>',
      '<span className="break-all font-mono">{path}</span>',
      '<span className="font-mono">{path}</span>',
      '<span className="truncate font-mono text-xs">{path}</span>',
      '<p>Delete "{name}"?</p>',
      '<p>Delete "<b>{name}</b>"?</p>',
      "<p>Delete '{name}'?</p>",
      "`dux will ask '${label}' to shut down`",
      "Delete &lsquo;{name}&rsquo; now.",
      "Delete ‘{name}’ now.",
      "Delete &quot;{name}&quot; now.",
      "Delete &#34;{name}&#34; now.",
      "Delete &#39;{name}&#39; now.",
      "Delete &apos;{name}&apos; now.",
    ]) {
      expect(nameDelimiterViolations(bad), bad).toHaveLength(1)
    }
  })

  it("leaves quoted UI words and the shared chip alone", () => {
    for (const fine of [
      "run “Reload config” afterwards.",
      "Tick “Use randomized pet name” to autofill",
      "Delete <InlineCode>{path}</InlineCode>?",
      '<p className="truncate font-mono text-sm">{destination}</p>',
      "The agent&apos;s folder stays where it is.",
      "Don't {verb} it.",
      'const label = "Reload config"',
    ]) {
      expect(nameDelimiterViolations(fine), fine).toEqual([])
    }
  })
})

describe("every name a modal shows is the shared chip", () => {
  const files = scannedFiles()

  it("scans the dialogs and the copy they render", () => {
    const names = files.map((f) => f.name)
    expect(files.length).toBeGreaterThan(30)
    expect(names).toContain("components/DeleteSessionDialog.tsx")
    // Found by the prose import, not by a list.
    expect(names).toContain("lib/addProjectWarning.ts")
    expect(names).toContain("components/createAgentDialogView.ts")
  })

  it("finds dialogs and copy builders by rule", () => {
    expect(isScanned("components/terminal/SomeDialog.tsx", "")).toBe(true)
    expect(isScanned("lib/newCopy.ts", 'import { chip } from "./prose"')).toBe(true)
    expect(isScanned("lib/newCopy.ts", 'import { chip } from "@/lib/prose"')).toBe(true)
    expect(isScanned("lib/unrelated.ts", 'import { x } from "./y"')).toBe(false)
    // A modal that is not named like one is still found by what it renders.
    expect(
      isScanned(
        "components/terminal/TakeOverCard.tsx",
        'import { Dialog, DialogContent } from "@/components/ui/dialog"',
      ),
    ).toBe(true)
    expect(
      isScanned(
        "components/ConfirmThing.tsx",
        'import {\n  AlertDialog,\n} from "@/components/ui/alert-dialog"',
      ),
    ).toBe(true)
    expect(
      isScanned("components/Other.tsx", 'import { x } from "@/components/ui/dialog-ish"'),
    ).toBe(false)
    expect(isScanned("components/SomeDialog.test.tsx", "")).toBe(false)
    expect(isScanned("lib/prose.tsx", "")).toBe(false)
  })

  it("has no quote delimiting a name and no hand-rolled chip", () => {
    const found = files.flatMap(({ name, source }) =>
      nameDelimiterViolations(source)
        .filter(
          (line) => !ALLOWED.some((a) => a.file === name && a.line === line),
        )
        .map((line) => `${name}: ${line}`),
    )
    expect(found).toEqual([])
  })

  it("keeps no stale exception", () => {
    for (const allowed of ALLOWED) {
      const file = files.find((f) => f.name === allowed.file)
      expect(file, allowed.file).toBeDefined()
      expect(nameDelimiterViolations(file!.source), allowed.file).toContain(
        allowed.line,
      )
      expect(allowed.reason.length).toBeGreaterThan(0)
    }
  })
})
