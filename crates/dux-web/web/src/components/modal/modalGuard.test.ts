// Every modal in the web UI is the one `Modal` component, which is what makes
// its rules (how it is presented, how it closes, what has focus when it opens,
// where its buttons go) impossible for a caller to forget. This guard reads the
// source and fails on any other way of building one: a file outside this
// module and the `ui/` primitives it wraps that imports the dialog or sheet
// primitives, or that hand-rolls a dialog's role.
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

const PRIMITIVE_IMPORT =
  /from\s+["'](?:@\/components\/ui\/(?:alert-)?(?:dialog|sheet)|(?:\.\.?\/)+(?:ui\/)?(?:alert-)?(?:dialog|sheet)|@base-ui\/react\/(?:alert-)?dialog)["']/
// A role written as an attribute. `[role="dialog"]` is a selector that looks
// for a modal, not one that builds it.
const HAND_ROLLED = /(?<!\[)\brole=["'](?:alert)?dialog["']|\baria-modal\b/

function buildsAModal(source: string): boolean {
  return PRIMITIVE_IMPORT.test(source) || HAND_ROLLED.test(source)
}

function isGuarded(name: string): boolean {
  if (!/\.tsx?$/.test(name) || name.includes(".test.")) return false
  return !/^(?:components\/modal|components\/ui|test)\//.test(name)
}

const NOT_MOVED = "Not yet moved onto Modal."

// Every file that still builds a modal another way, or that uses the
// primitives for something that is not a modal. An entry whose file no longer
// needs it fails the suite, so the list only shrinks.
const ALLOWED: Record<string, string> = {
  // Confirms.
  "components/ConfirmStopTabDialog.tsx": NOT_MOVED,
  "components/ConfirmForceReconnectDialog.tsx": NOT_MOVED,
  "components/ConfirmForceStopAgentDialog.tsx": NOT_MOVED,
  "components/ConfirmDetachAgentDialog.tsx": NOT_MOVED,
  "components/ConfirmCloseTabDialog.tsx": NOT_MOVED,
  "components/ConfirmDeleteTerminalDialog.tsx": NOT_MOVED,
  "components/ConfirmRecreateWorkingCopyDialog.tsx": NOT_MOVED,
  "components/DeleteSessionDialog.tsx": NOT_MOVED,
  "components/DeleteProjectDialog.tsx": NOT_MOVED,
  "components/RemoveProjectDialog.tsx": NOT_MOVED,
  "components/ConfirmUseExistingBranchDialog.tsx": NOT_MOVED,
  "components/ConfirmDiscardFileDialog.tsx": NOT_MOVED,
  "components/ConfirmDiscardFilesDialog.tsx": NOT_MOVED,
  "components/ConfirmCloseEditorTabDialog.tsx": NOT_MOVED,
  "components/ConfirmVanishedEditorDialog.tsx": NOT_MOVED,
  "components/ConfirmReloadFileDialog.tsx": NOT_MOVED,
  "components/ConfirmReopenEncodingDialog.tsx": NOT_MOVED,
  "components/DeleteEntryDialog.tsx": NOT_MOVED,
  "components/SaveConflictDialog.tsx": NOT_MOVED,
  "components/ConfirmDiscardEditsDialog.tsx":
    "Not yet moved. It is the discard confirm Modal itself raises for a form with unsaved edits.",
  "components/CheckoutDefaultBranchDialog.tsx": NOT_MOVED,
  // Reports.
  "components/TaskManagerDialog.tsx": NOT_MOVED,
  "components/AgentInfoDialog.tsx": NOT_MOVED,
  "components/ProjectInfoDialog.tsx": NOT_MOVED,
  "components/FileInfoDialog.tsx": NOT_MOVED,
  "components/StartupLogsDialog.tsx": NOT_MOVED,
  "components/CopiedTextDialog.tsx": NOT_MOVED,
  "components/FirstLoadDialog.tsx": NOT_MOVED,
  "components/PullRequestsDialog.tsx": NOT_MOVED,
  // Pickers.
  "components/ProjectsDialog.tsx": NOT_MOVED,
  "components/ProjectList.tsx":
    "The Projects list's header, which goes when ProjectsDialog moves; the New agent picker takes only its rows.",
  "components/ChangeBaseBranchDialog.tsx": NOT_MOVED,
  "components/StackNewDialog.tsx": NOT_MOVED,
  // Forms.
  "components/CreateAgentDialog.tsx": NOT_MOVED,
  "components/RenameSessionDialog.tsx": NOT_MOVED,
  "components/CloneProjectDialog.tsx": NOT_MOVED,
  "components/CommitDialog.tsx": NOT_MOVED,
  "components/ChangeProviderDialog.tsx": NOT_MOVED,
  "components/AttachPullRequestDialog.tsx": NOT_MOVED,
  "components/NewEntryDialog.tsx": NOT_MOVED,
  "components/RenameEntryDialog.tsx": NOT_MOVED,
  "components/GlobalEnvDialog.tsx": NOT_MOVED,
  "components/AgentEnvDialog.tsx": NOT_MOVED,
  "components/AgentStartupCommandDialog.tsx": NOT_MOVED,
  "components/ProjectSettingsDialog.tsx": NOT_MOVED,
  "components/ConfigEditorDialog.tsx": NOT_MOVED,
  "components/FormDialogContent.tsx":
    "The shell the forms not yet moved share; it goes with the last of them.",
  "components/AddProjectDialog.tsx": NOT_MOVED,
  "components/MoveEntryDialog.tsx": NOT_MOVED,
  "components/WorktreesDialog.tsx": NOT_MOVED,
  "components/MacrosDialog.tsx": NOT_MOVED,
  "components/CustomizeWebappDialog.tsx": NOT_MOVED,
  // Not modals of the four kinds.
  "components/EditorOverlay.tsx":
    "Not yet decided: a host for the whole editor at a URL position of its own, not a modal with parts.",
  "components/OfflineOverlay.tsx":
    "A cover nobody can dismiss, with no focus trap, no Escape and no history entry: not a modal of any kind.",
  "components/AppMenuSheet.tsx":
    "The phone's app menu: a menu drawn as a bottom sheet, not a modal.",
}

describe("the detector", () => {
  it("flags every way of building a modal without the Modal component", () => {
    for (const bad of [
      'import { Dialog, DialogContent } from "@/components/ui/dialog"',
      'import {\n  Sheet,\n  SheetContent,\n} from "@/components/ui/sheet"',
      'import { AlertDialog } from "@/components/ui/alert-dialog"',
      'import { DialogTitle } from "./ui/dialog"',
      'import { SheetContent } from "../ui/sheet"',
      'import { Dialog } from "@base-ui/react/dialog"',
      '<div role="dialog" aria-labelledby={id}>',
      "<div role='alertdialog'>",
      '<div aria-modal="true">',
    ]) {
      expect(buildsAModal(bad), bad).toBe(true)
    }
  })

  it("leaves the Modal component, lookalike modules and selectors alone", () => {
    for (const fine of [
      'import { Modal } from "@/components/modal"',
      'import { x } from "@/components/ui/dialog-ish"',
      'import { DropdownMenu } from "@/components/ui/dropdown-menu"',
      `const LAYER = '[role="dialog"], [role="alertdialog"]'`,
    ]) {
      expect(buildsAModal(fine), fine).toBe(false)
    }
  })

  it("guards every source file but the module, the primitives and tests", () => {
    expect(isGuarded("components/SomeDialog.tsx")).toBe(true)
    expect(isGuarded("components/terminal/SomeCard.tsx")).toBe(true)
    expect(isGuarded("lib/thing.ts")).toBe(true)
    expect(isGuarded("components/modal/Modal.tsx")).toBe(false)
    expect(isGuarded("components/ui/dialog.tsx")).toBe(false)
    expect(isGuarded("components/SomeDialog.test.tsx")).toBe(false)
  })
})

describe("a modal is built with the Modal component", () => {
  const offenders = walk(srcDir)
    .map((path) => relative(srcDir, path))
    .filter(isGuarded)
    .filter((name) => buildsAModal(readFileSync(join(srcDir, name), "utf-8")))

  it("and nowhere else, outside the files still to move", () => {
    expect(offenders.filter((name) => !(name in ALLOWED))).toEqual([])
  })

  it("keeps no entry for a file that no longer needs one", () => {
    expect(Object.keys(ALLOWED).filter((name) => !offenders.includes(name))).toEqual([])
    for (const reason of Object.values(ALLOWED)) expect(reason.length).toBeGreaterThan(0)
  })
})
