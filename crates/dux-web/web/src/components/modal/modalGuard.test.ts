// A modal in the web UI is built with the `Modal` component, which is what
// makes its rules (how it is presented, how it closes, what has focus when it
// opens, where its buttons go) impossible for a caller to forget. This guard
// reads the source and fails on one built any other way.
//
// It reads text, not the program, so it cannot see:
//   - a root reached through anything but an import, a `const`/`let`/`var`
//     alias of an imported name or `createElement` (a property of an object,
//     a function's argument, a re-export of a re-export it does not follow);
//   - a role written through anything but a literal or a constant holding one;
//   - a full-screen layer whose classes are not literals in one string or one
//     run of comma-separated strings (a conditional part, a variable, a
//     computed style);
//   - anything in a test file.
// And it cannot hold a number to "only goes down": the counts live in this
// file, so raising one is allowed, as a one-line change a reviewer sees.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs"
import { dirname, join, normalize, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const srcDir = fileURLToPath(new URL("../../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

/** The modules a modal can be built from, by the path an import reaches, and
 * the names in each that open one. "*" is every name the module exports. */
const MODAL_MODULES: Record<string, readonly string[] | "*"> = {
  "components/ui/dialog.tsx": ["Dialog"],
  "components/ui/sheet.tsx": ["Sheet"],
  "components/ui/command.tsx": ["CommandDialog"],
  "@base-ui/react/dialog": "*",
  "@base-ui/react/alert-dialog": "*",
  "@base-ui/react/drawer": "*",
  "@base-ui/react": ["Dialog", "AlertDialog", "Drawer"],
}

const EXTENSION = /\.(?:[cm]?[jt]sx?)$/

/** The module a specifier written in `file` reaches: a path under `src` for
 * one of ours, however it is spelled, and the specifier itself for a package. */
function resolveImport(srcDir: string, file: string, specifier: string): string {
  const local = specifier.startsWith("@/")
    ? specifier.slice(2)
    : specifier.startsWith(".")
      ? normalize(join(dirname(file), specifier))
      : null
  if (local === null) return specifier.replace(/\/index(?:\.[cm]?js)?$/, "")
  const bare = local.replace(EXTENSION, "")
  for (const candidate of [
    `${bare}.tsx`,
    `${bare}.ts`,
    join(bare, "index.tsx"),
    join(bare, "index.ts"),
  ]) {
    if (existsSync(join(srcDir, candidate))) return candidate.split("\\").join("/")
  }
  return bare.split("\\").join("/")
}

const IMPORT =
  /(?:import|export)\s+(?:type\s+)?([^"'`;]*?)\s*from\s*["'`]([^"'`]+)["'`]|import\s*\(\s*["'`]([^"'`]+)["'`]\s*\)|require\s*\(\s*["'`]([^"'`]+)["'`]\s*\)/g

const escape = (name: string) => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
const count = (source: string, pattern: RegExp) => [...source.matchAll(pattern)].length

// How many times a file opens a dialog or sheet root it imported, under
// whatever local name, a `const` alias of it or `createElement`. An import
// whose names cannot be read counts once.
function rootsOpened(srcDir: string, file: string, source: string): number {
  let opened = 0
  // `<Name` for a wrapper, `<Name.Root` for a base-ui namespace, and the same
  // name handed to `createElement`.
  const opens = (local: string, namespace: boolean): number => {
    const name = escape(local) + (namespace ? "\\.Root" : "")
    const tags = count(source, new RegExp(`<${name}(?=[\\s/>])`, "g"))
    const calls = count(source, new RegExp(`createElement\\(\\s*${name}(?![\\w$.])`, "g"))
    let aliased = 0
    for (const alias of source.matchAll(
      new RegExp(`\\b(?:const|let|var)\\s+([\\w$]+)\\s*=\\s*${escape(local)}(\\.Root)?(?![\\w$.])`, "g"),
    )) {
      aliased += opens(alias[1], namespace && alias[2] === undefined)
    }
    return tags + calls + aliased
  }
  for (const match of source.matchAll(IMPORT)) {
    const clause = match[1]
    const names = MODAL_MODULES[resolveImport(srcDir, file, match[2] ?? match[3] ?? match[4])]
    if (names === undefined) continue
    const isExport = match[0].startsWith("export")
    if (clause === undefined || isExport) {
      // A dynamic import, a require or a re-export hands the roots on unseen.
      opened += 1
      continue
    }
    const namespace = /\*\s+as\s+([\w$]+)/.exec(clause)?.[1]
    if (namespace) {
      opened += count(source, new RegExp(`<${escape(namespace)}\\.[\\w$.]+(?=[\\s/>])`, "g"))
    }
    const named = /\{([^}]*)\}/.exec(clause)?.[1] ?? ""
    for (const part of named.split(",")) {
      const [imported, local = imported] = part
        .replace(/^\s*type\s+/, "")
        .trim()
        .split(/\s+as\s+/)
      if (!imported) continue
      if (names !== "*" && !names.includes(imported)) continue
      // A base-ui namespace is opened as `<Local.Root>`, a wrapper as `<Local>`.
      opened += opens(
        local,
        names === "*" || resolveImport(srcDir, file, match[2]) === "@base-ui/react",
      )
    }
    const whole = /^\s*([\w$]+)\s*(?:,|$)/.exec(clause)?.[1]
    if (whole) opened += 1
  }
  return opened
}

// A dialog role or `aria-modal`, written as an attribute, an object property
// or a `setAttribute` call. `[role="dialog"]` is a selector that looks for a
// modal, not one that builds it.
const ROLE =
  /(?<![[\w])["'`]?role["'`]?\s*(?:[=:]\s*\{?|,)\s*["'`](?:alert)?dialog["'`]|(?<![[\w-])["'`]?aria-?modal["'`]?\s*[=:,]/gi

// A dialog role held in a constant, and every place that constant is a role.
function rolesFromConstants(source: string): number {
  let found = 0
  for (const held of source.matchAll(
    /\b(?:const|let|var)\s+([\w$]+)\s*=\s*["'`](?:alert)?dialog["'`]/g,
  )) {
    const name = escape(held[1])
    found += count(
      source,
      new RegExp(`(?<![[\\w])["'\`]?role["'\`]?\\s*(?:[=:]\\s*\\{?|,)\\s*${name}(?![\\w$])`, "g"),
    )
  }
  return found
}

// One class string, or one run of comma-separated strings (the arguments of
// `cn`), that pins a layer over the whole viewport.
const LITERAL = String.raw`["'\`][^"'\`]*["'\`]`
const RUN = String.raw`(?:\s*,\s*${LITERAL})*?\s*,\s*`
const FULL_SCREEN = new RegExp(
  [
    String.raw`["'\`][^"'\`]*\bfixed\b[^"'\`]*\binset-0\b[^"'\`]*["'\`]`,
    String.raw`["'\`][^"'\`]*\binset-0\b[^"'\`]*\bfixed\b[^"'\`]*["'\`]`,
    String.raw`["'\`][^"'\`]*\bfixed\b[^"'\`]*["'\`]${RUN}["'\`][^"'\`]*\binset-0\b[^"'\`]*["'\`]`,
    String.raw`["'\`][^"'\`]*\binset-0\b[^"'\`]*["'\`]${RUN}["'\`][^"'\`]*\bfixed\b[^"'\`]*["'\`]`,
  ].join("|"),
  "g",
)

interface ModalMarks {
  /** Dialog or sheet roots opened. */
  roots: number
  /** Dialog roles and `aria-modal`s written by hand. */
  roles: number
  /** Layers pinned over the whole viewport. */
  layers: number
}

function modalMarks(srcDir: string, file: string, source: string): ModalMarks {
  return {
    roots: rootsOpened(srcDir, file, source),
    roles: count(source, ROLE) + rolesFromConstants(source),
    layers: count(source, FULL_SCREEN),
  }
}

/** Whether the file imports anything from a module a modal is built from,
 * a part with no root included. */
function touchesPrimitive(srcDir: string, file: string, source: string): boolean {
  for (const match of source.matchAll(IMPORT)) {
    const reached = resolveImport(srcDir, file, match[2] ?? match[3] ?? match[4])
    if (reached === "components/ui/command.tsx") continue
    if (reached in MODAL_MODULES) return true
  }
  return false
}

// The only files that may build a modal from the primitives: the two
// primitives themselves and the component that wraps them.
const PRIMITIVES = ["components/ui/dialog.tsx", "components/ui/sheet.tsx"]
const MODAL_COMPONENT = ["components/modal/Modal.tsx"]

function isGuarded(name: string): boolean {
  if (!EXTENSION.test(name) || name.endsWith(".d.ts") || name.includes(".test.")) return false
  return !PRIMITIVES.includes(name) && !MODAL_COMPONENT.includes(name)
}

const marksOf = (name: string, source: string): ModalMarks => modalMarks(srcDir, name, source)
const total = (marks: ModalMarks) => marks.roots + marks.roles + marks.layers

interface Entry extends ModalMarks {
  why?: string
}

const NONE = { roots: 0, roles: 0, layers: 0 } as const
const ROOT = { roots: 1, roles: 0, layers: 0 } as const
const ROOTS = (roots: number) => ({ roots, roles: 0, layers: 0 })

// The files that still build a modal another way. Each pins its marks of each
// kind, so a modal added to a listed file fails as one added to a new file
// does, and so does trading one kind of mark for another; an entry whose file
// has moved or changed fails until the entry follows it.
const STILL_TO_MOVE: Record<string, Entry> = {
  "components/AddProjectDialog.tsx": { ...ROOT },
  "components/CheckoutDefaultBranchDialog.tsx": { ...ROOT },
  "components/ConfigEditorDialog.tsx": { ...ROOT },
  "components/ConfirmCloseEditorTabDialog.tsx": { ...ROOT },
  "components/ConfirmCloseTabDialog.tsx": { ...ROOT },
  "components/ConfirmDeleteTerminalDialog.tsx": { ...ROOT },
  "components/ConfirmDetachAgentDialog.tsx": { ...ROOT },
  "components/ConfirmDiscardFileDialog.tsx": { ...ROOT },
  "components/ConfirmDiscardFilesDialog.tsx": { ...ROOT },
  "components/ConfirmForceReconnectDialog.tsx": { ...ROOT },
  "components/ConfirmForceStopAgentDialog.tsx": { ...ROOT },
  "components/ConfirmRecreateWorkingCopyDialog.tsx": { ...ROOT },
  "components/ConfirmReloadFileDialog.tsx": { ...ROOT },
  "components/ConfirmReopenEncodingDialog.tsx": { ...ROOT },
  "components/ConfirmStopTabDialog.tsx": { ...ROOT },
  "components/ConfirmUseExistingBranchDialog.tsx": { ...ROOT },
  "components/ConfirmVanishedEditorDialog.tsx": { ...ROOT },
  "components/CustomizeWebappDialog.tsx": { ...ROOTS(3) },
  "components/DeleteEntryDialog.tsx": { ...ROOT },
  "components/DeleteProjectDialog.tsx": { ...ROOT },
  "components/DeleteSessionDialog.tsx": { ...ROOT },
  "components/FirstLoadDialog.tsx": { ...ROOT },
  "components/FormDialogContent.tsx": { ...NONE, why: "The shell the forms not yet moved share; it goes with the last of them." },
  "components/MacrosDialog.tsx": { ...ROOTS(3) },
  "components/MoveEntryDialog.tsx": { ...ROOT },
  "components/PullRequestsDialog.tsx": { ...ROOTS(2) },
  "components/RemoveProjectDialog.tsx": { ...ROOT },
  "components/SaveConflictDialog.tsx": { ...ROOT },
  "components/TaskManagerDialog.tsx": { ...ROOTS(2) },
  "components/WorktreesDialog.tsx": { ...ROOTS(3) },
  "components/ui/command.tsx": { ...ROOT, why: "CommandDialog, which only its own test mounts: delete it or move it." },
}

// Surfaces that are deliberately not modals of the four kinds, by name. One
// the detector cannot see names what to find in its file instead.
const NOT_MODALS: Record<string, Entry & { file: string; why: string; names?: string }> = {
  "the offline cover": {
    file: "components/OfflineOverlay.tsx",
    roots: 0, roles: 2, layers: 1,
    why: "Nobody can dismiss it: no focus trap, no Escape, no history entry. It lifts when the connection is back.",
  },
  "the auth gate": {
    file: "components/AuthGate.tsx",
    roots: 0, roles: 0, layers: 1,
    why: "A page in place of the app (sign-in, first password, unreachable), with the app inert under it; it has no dialog role, so only its full-screen layer is seen.",
  },
  "the editor overlay": {
    file: "components/EditorOverlay.tsx",
    ...ROOT,
    why: "A host for the whole editor at a URL position of its own, 80rem wide: not a modal with parts.",
  },
  "the phone's app menu": {
    file: "components/AppMenuSheet.tsx",
    ...ROOT,
    why: "A menu drawn as a bottom sheet.",
  },
  "menus and popovers as bottom sheets": {
    file: "components/ui/popupSheet.ts",
    roots: 0, roles: 0, layers: 1,
    why: "The backdrop the dropdown, context menu and popover primitives share on a phone.",
  },
  "the phone's sidebar drawer": {
    file: "components/ui/sidebar.tsx",
    ...ROOT,
    why: "Navigation chrome that slides in from the side.",
  },
  "the take-over card": {
    file: "components/terminal/partPieces.tsx",
    ...NONE,
    names: "function TakeoverCard(",
    why: "A cover inside one pane, over a terminal this device does not drive; the page around it stays live.",
  },
}

const listed = (name: string): Entry | undefined =>
  STILL_TO_MOVE[name] ?? Object.values(NOT_MODALS).find((entry) => entry.file === name)

describe("the detector", () => {
  const marks = (source: string, file = "components/Some.tsx") => total(marksOf(file, source))

  it("follows an import to the module it reaches, however it is spelled", () => {
    for (const [file, specifier] of [
      ["components/Some.tsx", "@/components/ui/dialog"],
      ["components/Some.tsx", "./ui/dialog"],
      ["components/terminal/Some.tsx", "../ui/dialog"],
      ["App.tsx", "./components/ui/dialog"],
      ["hooks/use-some.ts", "../components/ui/dialog"],
      ["lib/some.ts", "../components/ui/dialog.tsx"],
      ["lib/some.ts", "@/components/ui/dialog.js"],
    ]) {
      expect(resolveImport(srcDir, file, specifier), `${specifier} in ${file}`).toBe(
        "components/ui/dialog.tsx",
      )
    }
    expect(resolveImport(srcDir, "lib/some.ts", "@/components/ui/dialog-ish")).not.toBe(
      "components/ui/dialog.tsx",
    )
  })

  it("counts every root a file opens, under whatever name it imported it", () => {
    for (const [bad, expected] of [
      ['import { Dialog, DialogContent } from "@/components/ui/dialog"\n<Dialog open><DialogContent /></Dialog>', 1],
      ['import {\n  Sheet as Drawer,\n  SheetContent,\n} from "./ui/sheet"\n<Drawer /><Drawer />', 2],
      ['import { CommandDialog } from "@/components/ui/command"\n<CommandDialog />', 1],
      ['import { Dialog } from "@base-ui/react/dialog"\n<Dialog.Root><Dialog.Popup /></Dialog.Root>', 1],
      ['import { Dialog as D } from "@base-ui/react"\n<D.Root />', 1],
      ['import * as Parts from "@/components/ui/dialog"\n<Parts.Dialog />', 1],
      ['const parts = await import("@/components/ui/sheet")', 1],
      ['export { Dialog } from "@/components/ui/dialog"', 1],
      ['import { Drawer } from "@base-ui/react/drawer"\n<Drawer.Root />', 1],
      ['import { Drawer } from "@base-ui/react"\n<Drawer.Root />', 1],
      ['import { Dialog } from "@/components/ui/dialog"\nconst D = Dialog\n<D open />', 1],
      ['import { Dialog } from "@base-ui/react/dialog"\nconst R = Dialog.Root\n<R />', 1],
      ['import { Dialog } from "@/components/ui/dialog"\ncreateElement(Dialog, { open: true })', 1],
      ['import { Dialog } from "@base-ui/react/dialog"\nReact.createElement(Dialog.Root, {})', 1],
    ] as const) {
      expect(marks(bad), bad).toBe(expected)
    }
  })

  it("counts a dialog role written by hand in any form, and a full-screen layer", () => {
    for (const bad of [
      '<div role="dialog" aria-labelledby={id}>',
      "<div role='alertdialog'>",
      '<div role={"dialog"}>',
      'const props = { role: "dialog" }',
      'el.setAttribute("role", "alertdialog")',
      '<div aria-modal="true">',
      'const props = { "aria-modal": true }',
      '<div className="fixed inset-0 z-[300] bg-background">',
      'const COVER = "inset-0 z-10 fixed"',
      '<div className={cn("fixed", "inset-0")}>',
      'cn("inset-0 z-50", "bg-background", "fixed")',
      'const ROLE = "alertdialog"\n<div role={ROLE}>',
    ]) {
      expect(marks(bad), bad).toBeGreaterThan(0)
    }
  })

  it("leaves the Modal component, lookalike modules, parts and selectors alone", () => {
    for (const fine of [
      'import { Modal } from "@/components/modal"\n<Modal kind="report" />',
      'import { x } from "@/components/ui/dialog-ish"\n<x />',
      'import { Command } from "@/components/ui/command"\n<Command />',
      'import { DropdownMenu } from "@/components/ui/dropdown-menu"\n<DropdownMenu />',
      `const LAYER = '[role="dialog"], [role="alertdialog"]'`,
      '<div className="absolute inset-0">',
      '<div role="dialogue-box">',
      'cn("fixed top-0", "right-0")',
    ]) {
      expect(marks(fine), fine).toBe(0)
    }
  })

  it("sees a file that takes only a part from a primitive", () => {
    const source = 'import { DialogTitle } from "@/components/ui/dialog"\n<DialogTitle />'
    expect(marks(source)).toBe(0)
    expect(touchesPrimitive(srcDir, "components/Some.tsx", source)).toBe(true)
  })

  it("guards every source file but the two primitives, the Modal component and tests", () => {
    expect(isGuarded("components/SomeDialog.tsx")).toBe(true)
    expect(isGuarded("components/modal/SomethingNew.tsx")).toBe(true)
    expect(isGuarded("test/someHelper.tsx")).toBe(true)
    expect(isGuarded("lib/legacy.js")).toBe(true)
    expect(isGuarded("components/Old.jsx")).toBe(true)
    expect(isGuarded("components/terminal/SomeCard.tsx")).toBe(true)
    expect(isGuarded("components/ui/command.tsx")).toBe(true)
    expect(isGuarded("components/ui/new-wrapper.tsx")).toBe(true)
    expect(isGuarded("App.tsx")).toBe(true)
    expect(isGuarded("lib/thing.ts")).toBe(true)
    expect(isGuarded("components/modal/Modal.tsx")).toBe(false)
    expect(isGuarded("components/ui/dialog.tsx")).toBe(false)
    expect(isGuarded("components/SomeDialog.test.tsx")).toBe(false)
  })
})

describe("a modal is built with the Modal component", () => {
  const found = new Map<string, ModalMarks>()
  for (const path of walk(srcDir)) {
    const name = relative(srcDir, path).split("\\").join("/")
    if (!isGuarded(name)) continue
    const source = readFileSync(path, "utf-8")
    const marks = marksOf(name, source)
    if (total(marks) > 0 || touchesPrimitive(srcDir, name, source)) found.set(name, marks)
  }
  const kinds = ["roots", "roles", "layers"] as const

  it("and nowhere else: no file outside the lists builds one", () => {
    const unlisted = [...found.keys()].filter((name) => listed(name) === undefined)
    expect(
      unlisted,
      "These files build a modal without the Modal component. Build it with <Modal kind=…> from @/components/modal.",
    ).toEqual([])
  })

  it("and a listed file carries exactly the marks of each kind it is listed with", () => {
    const changed = [...found].flatMap(([name, marks]) => {
      const entry = listed(name)
      if (entry === undefined) return []
      return kinds
        .filter((kind) => entry[kind] !== marks[kind])
        .map((kind) => `${name}: ${kind} listed ${entry[kind]}, found ${marks[kind]}`)
    })
    expect(
      changed,
      "A listed file's marks changed. More: a modal was added without the Modal component; build it with <Modal>. Fewer: lower the entry with it.",
    ).toEqual([])
  })

  it("keeps no entry for a file that no longer needs one", () => {
    const stale = Object.keys(STILL_TO_MOVE).filter((name) => !found.has(name))
    expect(stale, "Moved or gone: delete the entry.").toEqual([])
    for (const [name, entry] of Object.entries(NOT_MODALS)) {
      expect(existsSync(join(srcDir, entry.file)), `${name}: ${entry.file} is gone`).toBe(true)
      const source = readFileSync(join(srcDir, entry.file), "utf-8")
      const marks = marksOf(entry.file, source)
      for (const kind of kinds) expect(marks[kind], `${name}: ${kind}`).toBe(entry[kind])
      if (entry.names) expect(source, `${name} is no longer in ${entry.file}`).toContain(entry.names)
      expect(entry.why.length, name).toBeGreaterThan(0)
    }
  })
})
