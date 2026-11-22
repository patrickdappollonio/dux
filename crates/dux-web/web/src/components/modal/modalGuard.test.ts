// A modal in the web UI is built with the `Modal` component, which is what
// makes its rules (how it is presented, how it closes, what has focus when it
// opens, where its buttons go) impossible for a caller to forget. This guard
// reads the source and fails on one built any other way.
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
  "@base-ui/react": ["Dialog", "AlertDialog"],
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
// whatever local name. An import whose names cannot be read counts once.
function rootsOpened(srcDir: string, file: string, source: string): number {
  let opened = 0
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
      const tag = names === "*" || resolveImport(srcDir, file, match[2]) === "@base-ui/react"
        ? `<${escape(local)}\\.Root(?=[\\s/>])`
        : `<${escape(local)}(?=[\\s/>])`
      opened += count(source, new RegExp(tag, "g"))
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

// One class string that pins a layer over the whole viewport.
const FULL_SCREEN = /["'`][^"'`]*\bfixed\b[^"'`]*\binset-0\b[^"'`]*["'`]|["'`][^"'`]*\binset-0\b[^"'`]*\bfixed\b[^"'`]*["'`]/g

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
    roles: count(source, ROLE),
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
// primitives themselves and this module, which wraps them.
const PRIMITIVES = ["components/ui/dialog.tsx", "components/ui/sheet.tsx"]

function isGuarded(name: string): boolean {
  if (!/\.tsx?$/.test(name) || name.includes(".test.")) return false
  if (PRIMITIVES.includes(name)) return false
  return !/^(?:components\/modal|test)\//.test(name)
}

const marksOf = (name: string, source: string) => {
  const marks = modalMarks(srcDir, name, source)
  return marks.roots + marks.roles + marks.layers
}

interface Entry {
  /** How many marks of a modal the file carries today: dialog or sheet roots
   * it opens, dialog roles written by hand, full-screen layers. */
  marks: number
  why?: string
}

// The files that still build a modal another way. Each pins its marks, so a
// modal added to a listed file fails as one added to a new file does, and an
// entry whose file has moved or changed fails until the entry follows it.
const STILL_TO_MOVE: Record<string, Entry> = {
  "components/AddProjectDialog.tsx": { marks: 1 },
  "components/AgentEnvDialog.tsx": { marks: 1 },
  "components/AgentInfoDialog.tsx": { marks: 1 },
  "components/AgentStartupCommandDialog.tsx": { marks: 1 },
  "components/AttachPullRequestDialog.tsx": { marks: 2 },
  "components/ChangeBaseBranchDialog.tsx": { marks: 2 },
  "components/ChangeProviderDialog.tsx": { marks: 1 },
  "components/CheckoutDefaultBranchDialog.tsx": { marks: 1 },
  "components/CloneProjectDialog.tsx": { marks: 1 },
  "components/CommitDialog.tsx": { marks: 1 },
  "components/ConfigEditorDialog.tsx": { marks: 1 },
  "components/ConfirmCloseEditorTabDialog.tsx": { marks: 1 },
  "components/ConfirmCloseTabDialog.tsx": { marks: 1 },
  "components/ConfirmDeleteTerminalDialog.tsx": { marks: 1 },
  "components/ConfirmDetachAgentDialog.tsx": { marks: 1 },
  "components/ConfirmDiscardFileDialog.tsx": { marks: 1 },
  "components/ConfirmDiscardFilesDialog.tsx": { marks: 1 },
  "components/ConfirmForceReconnectDialog.tsx": { marks: 1 },
  "components/ConfirmForceStopAgentDialog.tsx": { marks: 1 },
  "components/ConfirmRecreateWorkingCopyDialog.tsx": { marks: 1 },
  "components/ConfirmReloadFileDialog.tsx": { marks: 1 },
  "components/ConfirmReopenEncodingDialog.tsx": { marks: 1 },
  "components/ConfirmStopTabDialog.tsx": { marks: 1 },
  "components/ConfirmUseExistingBranchDialog.tsx": { marks: 1 },
  "components/ConfirmVanishedEditorDialog.tsx": { marks: 1 },
  "components/CopiedTextDialog.tsx": { marks: 1 },
  "components/CreateAgentDialog.tsx": { marks: 1 },
  "components/CustomizeWebappDialog.tsx": { marks: 3 },
  "components/DeleteEntryDialog.tsx": { marks: 1 },
  "components/DeleteProjectDialog.tsx": { marks: 1 },
  "components/DeleteSessionDialog.tsx": { marks: 1 },
  "components/FileInfoDialog.tsx": { marks: 1 },
  "components/FirstLoadDialog.tsx": { marks: 1 },
  "components/FormDialogContent.tsx": { marks: 0, why: "The shell the forms not yet moved share; it goes with the last of them." },
  "components/GlobalEnvDialog.tsx": { marks: 1 },
  "components/MacrosDialog.tsx": { marks: 3 },
  "components/MoveEntryDialog.tsx": { marks: 1 },
  "components/NewEntryDialog.tsx": { marks: 1 },
  "components/ProjectInfoDialog.tsx": { marks: 1 },
  "components/ProjectList.tsx": { marks: 0, why: "The Projects list's header, which goes when ProjectsDialog moves; the New agent picker takes only its rows." },
  "components/ProjectSettingsDialog.tsx": { marks: 1 },
  "components/ProjectsDialog.tsx": { marks: 1 },
  "components/PullRequestsDialog.tsx": { marks: 2 },
  "components/RemoveProjectDialog.tsx": { marks: 1 },
  "components/RenameEntryDialog.tsx": { marks: 1 },
  "components/RenameSessionDialog.tsx": { marks: 1 },
  "components/SaveConflictDialog.tsx": { marks: 1 },
  "components/StackNewDialog.tsx": { marks: 1 },
  "components/StartupLogsDialog.tsx": { marks: 1 },
  "components/TaskManagerDialog.tsx": { marks: 2 },
  "components/WorktreesDialog.tsx": { marks: 3 },
  "components/ui/command.tsx": { marks: 1, why: "CommandDialog, which only its own test mounts: delete it or move it." },
}

// The size of the list above. THESE NUMBERS ONLY GO DOWN: a modal is moved
// onto `Modal` and its entry and marks leave. Raising one is adding a modal
// built another way, which is what this guard exists to stop.
const STILL_TO_MOVE_SIZE = { files: 52, marks: 60 }

// Surfaces that are deliberately not modals of the four kinds, by name. One
// the detector cannot see names what to find in its file instead.
const NOT_MODALS: Record<string, Entry & { file: string; why: string; names?: string }> = {
  "the offline cover": {
    file: "components/OfflineOverlay.tsx",
    marks: 3,
    why: "Nobody can dismiss it: no focus trap, no Escape, no history entry. It lifts when the connection is back.",
  },
  "the auth gate": {
    file: "components/AuthGate.tsx",
    marks: 1,
    why: "A page in place of the app (sign-in, first password, unreachable), with the app inert under it; it has no dialog role, so only its full-screen layer is seen.",
  },
  "the editor overlay": {
    file: "components/EditorOverlay.tsx",
    marks: 1,
    why: "A host for the whole editor at a URL position of its own, 80rem wide: not a modal with parts.",
  },
  "the phone's app menu": {
    file: "components/AppMenuSheet.tsx",
    marks: 1,
    why: "A menu drawn as a bottom sheet.",
  },
  "menus and popovers as bottom sheets": {
    file: "components/ui/popupSheet.ts",
    marks: 1,
    why: "The backdrop the dropdown, context menu and popover primitives share on a phone.",
  },
  "the phone's sidebar drawer": {
    file: "components/ui/sidebar.tsx",
    marks: 1,
    why: "Navigation chrome that slides in from the side.",
  },
  "the take-over card": {
    file: "components/terminal/partPieces.tsx",
    marks: 0,
    names: "function TakeoverCard(",
    why: "A cover inside one pane, over a terminal this device does not drive; the page around it stays live.",
  },
}

const listed = (name: string): Entry | undefined =>
  STILL_TO_MOVE[name] ?? Object.values(NOT_MODALS).find((entry) => entry.file === name)

describe("the detector", () => {
  const marks = (source: string, file = "components/Some.tsx") => marksOf(file, source)

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
    ]) {
      expect(marks(fine), fine).toBe(0)
    }
  })

  it("sees a file that takes only a part from a primitive", () => {
    const source = 'import { DialogTitle } from "@/components/ui/dialog"\n<DialogTitle />'
    expect(marks(source)).toBe(0)
    expect(touchesPrimitive(srcDir, "components/Some.tsx", source)).toBe(true)
  })

  it("guards every source file but the two primitives, this module and tests", () => {
    expect(isGuarded("components/SomeDialog.tsx")).toBe(true)
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
  const found = new Map<string, number>()
  for (const path of walk(srcDir)) {
    const name = relative(srcDir, path).split("\\").join("/")
    if (!isGuarded(name)) continue
    const source = readFileSync(path, "utf-8")
    const marks = marksOf(name, source)
    if (marks > 0 || touchesPrimitive(srcDir, name, source)) found.set(name, marks)
  }

  it("and nowhere else: no file outside the lists builds one", () => {
    const unlisted = [...found.keys()].filter((name) => listed(name) === undefined)
    expect(
      unlisted,
      "These files build a modal without the Modal component. Build it with <Modal kind=…> from @/components/modal.",
    ).toEqual([])
  })

  it("and a listed file builds no more of them than it did", () => {
    const changed = [...found]
      .filter(([name, marks]) => listed(name) !== undefined && listed(name)!.marks !== marks)
      .map(([name, marks]) => `${name}: listed ${listed(name)!.marks}, found ${marks}`)
    expect(
      changed,
      "A listed file's marks changed. More: a modal was added without the Modal component; build it with <Modal>. Fewer: lower the entry and STILL_TO_MOVE_SIZE with it.",
    ).toEqual([])
  })

  it("keeps no entry for a file that no longer needs one", () => {
    const stale = Object.keys(STILL_TO_MOVE).filter((name) => !found.has(name))
    expect(stale, "Moved or gone: delete the entry and lower STILL_TO_MOVE_SIZE.").toEqual([])
    for (const [name, entry] of Object.entries(NOT_MODALS)) {
      expect(existsSync(join(srcDir, entry.file)), `${name}: ${entry.file} is gone`).toBe(true)
      const source = readFileSync(join(srcDir, entry.file), "utf-8")
      expect(marksOf(entry.file, source), name).toBe(entry.marks)
      if (entry.names) expect(source, `${name} is no longer in ${entry.file}`).toContain(entry.names)
      expect(entry.why.length, name).toBeGreaterThan(0)
    }
  })

  it("has a list of files still to move that only shrinks", () => {
    const entries = Object.values(STILL_TO_MOVE)
    expect(
      { files: entries.length, marks: entries.reduce((sum, entry) => sum + entry.marks, 0) },
      "STILL_TO_MOVE_SIZE only goes down. If the list grew, a modal was added without the Modal component: build it with <Modal> rather than raising the number.",
    ).toEqual(STILL_TO_MOVE_SIZE)
  })
})
