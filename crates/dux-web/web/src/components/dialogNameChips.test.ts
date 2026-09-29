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

// The same chip one level up: StartTruncatedText renders its own span, so a
// monospace class handed to it is a monospace span all the same. Its props may
// span lines, so the element is matched up to its className, and the line
// reported is the className's own.
const MONO_START_TRUNCATED =
  /<StartTruncatedText\b[^>]*?\bclassName="[^"]*\bfont-mono\b[^"]*"/g

function nameDelimiterViolations(source: string): string[] {
  return [
    ...[...source.matchAll(QUOTE_BEFORE_NAME)].map((m) => m.index ?? 0),
    ...[...source.matchAll(HAND_ROLLED_CHIP)].map((m) => m.index ?? 0),
    ...[...source.matchAll(MONO_START_TRUNCATED)].map(
      (m) => (m.index ?? 0) + m[0].lastIndexOf("className="),
    ),
  ].map((index) => lineAround(source, index))
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
  {
    file: "components/ProjectsDialog.tsx",
    line: '<StartTruncatedText text={project.path} className="font-mono" tooltip />',
    reason: "A project row's folder on its second line: part of a list row, not a name in a sentence.",
  },
  // The files below are scanned because they build toast prose, not because
  // they are dialogs; each line is something other than a name in a sentence.
  {
    file: "components/EditorBody.tsx",
    line: 'className="flex-1 font-mono text-sm"',
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
      // The start-ellipsizing text component renders a span of its own, so a
      // monospace one is the same hand-rolled chip one level up.
      '<StartTruncatedText text={path} className="font-mono" />',
      '<StartTruncatedText text={path} className="flex-1 font-mono text-sm" />',
      '<StartTruncatedText\n  text={path}\n  className="font-mono text-xs"\n/>',
    ]) {
      expect(nameDelimiterViolations(bad), bad).toHaveLength(1)
    }
  })

  // The line reported, and matched against the allowlist, is the one carrying
  // the monospace class, so an exception names exactly what it excuses.
  it("reports a multi-line StartTruncatedText by its className line", () => {
    expect(
      nameDelimiterViolations(
        '<StartTruncatedText\n  text={path}\n  className="flex-1 font-mono text-sm"\n/>',
      ),
    ).toEqual(['className="flex-1 font-mono text-sm"'])
  })

  it("leaves a StartTruncatedText without a monospace class alone", () => {
    expect(
      nameDelimiterViolations('<StartTruncatedText text={name} className="text-sm" />'),
    ).toEqual([])
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

// A path is a name, and a name in a dialog is a chip. The shape that slips past
// the quote detector is a path interpolated into a plain template string that
// a dialog then prints as text: nothing quotes it, and nothing chips it either.
// This reads the dialogs and every lib/ module a dialog imports (where the
// copy a dialog prints is often built), and flags a template literal that
// interpolates a `.path` or a `path` into words: a template whose own text
// reads as a sentence (two words in a row). A `prose` template, a `chip(...)`
// argument and a template that is the child of an `<InlineCode>` are the chip
// itself, and a template with no words (a URL, a key, a shell argument) is not
// copy at all.
const PLAIN_PATH_TEMPLATE =
  /(?<!prose|chip\(|InlineCode>\{)`[^`]*\$\{[^}]*\bpath\b[^}]*\}[^`]*`/g

function plainPathTemplates(source: string): string[] {
  return [...source.matchAll(PLAIN_PATH_TEMPLATE)]
    .filter((m) => /[A-Za-z]{2,} [A-Za-z]{2,}/.test(m[0].replace(/\$\{[^}]*\}/g, "")))
    .map((m) => lineAround(source, m.index ?? 0))
}

// The lib modules the dialogs import, found from their import lines.
function dialogLibModules(): { name: string; source: string }[] {
  const dialogs = [...walk(join(srcDir, "components"))]
    .map((path) => ({ name: relative(srcDir, path), source: readFileSync(path, "utf-8") }))
    .filter(
      ({ name, source }) =>
        !name.includes(".test.") &&
        ((/Dialog[^/]*\.tsx$/.test(name) && name.startsWith("components/")) ||
          DIALOG_IMPORT.test(source)),
    )
  const imported = new Set<string>()
  for (const { source } of dialogs) {
    for (const m of source.matchAll(/from\s+["']@\/lib\/([\w./-]+)["']/g)) {
      imported.add(m[1]!)
    }
  }
  return [...imported].flatMap((module) => {
    for (const ext of [".ts", ".tsx"]) {
      try {
        const name = `lib/${module}${ext}`
        return [{ name, source: readFileSync(join(srcDir, name), "utf-8") }]
      } catch {
        // Not this extension.
      }
    }
    return []
  })
}

// Paths in templates that are not dialog copy: URLs, keys and file names
// handed to APIs, never shown in a dialog body. Each names why.
const PLAIN_PATH_ALLOWED: { file: string; line: string; reason: string }[] = []

describe("a path in dialog copy is a chip, not a plain template", () => {
  it("flags a path interpolated into a plain template string", () => {
    expect(plainPathTemplates("return `${file.path}/ is kept here`")).toHaveLength(1)
    expect(plainPathTemplates("return prose`${chip(file.path)} is a worktree`")).toEqual([])
    expect(plainPathTemplates("chip(`${row.path}/`)")).toEqual([])
  })

  it("finds none in the dialogs or the modules they import", () => {
    const found = [...scannedFiles(), ...dialogLibModules()].flatMap(({ name, source }) =>
      plainPathTemplates(source)
        .filter((line) => !PLAIN_PATH_ALLOWED.some((a) => a.file === name && a.line === line))
        .map((line) => `${name}: ${line}`),
    )
    expect([...new Set(found)]).toEqual([])
  })
})
