import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

// Sites whose only touch rule is a class the markup renders in every test
// harness differently (or not at all). jsdom cannot evaluate media queries, so
// the class text is what is pinned: the narrow-screen half and the coarse-pointer
// half must both be on the control, or a touch tablet gets the mouse size.

const read = (file: string) =>
  readFileSync(new URL(`./${file}`, import.meta.url), "utf8")

// The element holding `marker`: from the nearest `<tag` before it to the next
// closing tag (or, for a `const`, the rest of its line).
function openingTag(src: string, marker: string, tag: string): string {
  const at = src.indexOf(marker)
  expect(at, `${marker} is missing`).toBeGreaterThan(-1)
  if (tag === "const") return src.slice(at, src.indexOf("\n", at))
  const start = src.lastIndexOf(tag, at)
  const end = src.indexOf("</", at)
  return src.slice(start, end)
}

// Every quoted/template class string in `src` containing `needle`.
function classStringsWith(src: string, needle: string): string[] {
  return (src.match(/(["`])(?:(?!\1)[^\n])*\1/g) ?? []).filter((s) =>
    s.includes(needle),
  )
}

const BUTTON_FLOOR = ["max-md:min-h-10", "pointer-coarse:min-h-10"]

describe("every narrow-screen touch size has its coarse-pointer twin", () => {
  const files = [
    "AgentNotFound.tsx",
    "RouteNotFoundScreen.tsx",
    "DormantTabCard.tsx",
    "DormantTabSurface.tsx",
    "AddProjectDialog.tsx",
    "CreateAgentDialog.tsx",
    "CloneProjectDialog.tsx",
    "NewLauncherDialog.tsx",
    "ProjectList.tsx",
    "modal/Modal.tsx",
    "modal/ModalFooter.tsx",
    "CustomizeWebappDialog.tsx",
    "WorktreesDialog.tsx",
    "MoveEntryDialog.tsx",
    "ChangeBaseBranchDialog.tsx",
    "FolderBrowseList.tsx",
    "MacrosDialog.tsx",
  ]
  for (const file of files) {
    it(`${file} gives a finger what a phone gets`, () => {
      const src = read(file)
      for (const [narrow, coarse] of [
        ["max-md:min-h-10", "pointer-coarse:min-h-10"],
        ["max-md:min-h-11", "pointer-coarse:min-h-11"],
        ["max-md:size-10", "pointer-coarse:size-10"],
      ]) {
        for (const cls of classStringsWith(src, narrow)) {
          expect(cls, `${narrow} without ${coarse}`).toContain(coarse)
        }
      }
    })
  }

  it("keeps the picker rows tall for a finger at any width and compact only for a mouse", () => {
    expect(
      read("FolderBrowseList.tsx"),
      "a folder row is still compact at tablet width",
    ).not.toMatch(/(?<![:\w-])md:min-h-0/)
    for (const file of ["FolderBrowseList.tsx", "WorktreesDialog.tsx"]) {
      const src = read(file)
      expect(src).toContain("md:pointer-coarse:min-h-10")
      expect(src).toContain("md:not-pointer-coarse:min-h-0")
    }
  })
})

describe("controls that had no touch size at all", () => {
  const cases: [string, string, string, string[]][] = [
    ["MacrosDialog.tsx", "onClick={dismissMacrosConflict}", "<Button", BUTTON_FLOOR],
    ["MacrosDialog.tsx", "onClick={() => void reloadMacrosDraft()}", "<Button", BUTTON_FLOOR],
    ["MacrosDialog.tsx", "aria-label={`Actions for ${name}`}", "<Button", ["max-md:size-10", "pointer-coarse:size-10"]],
    ["MacrosDialog.tsx", "onClick={() => showProblem(shownProblem.key)}", "<Button", BUTTON_FLOOR],
    ["GlobalEnvDialog.tsx", "onClick={dismissGlobalEnvConflict}", "<Button", BUTTON_FLOOR],
    ["GlobalEnvDialog.tsx", "onClick={() => void reloadGlobalEnvDraft()}", "<Button", BUTTON_FLOOR],
    ["ConfigEditorDialog.tsx", "onClick={dismissConfigEditorConflict}", "<Button", BUTTON_FLOOR],
    ["ConfigEditorDialog.tsx", "openConfigEditor()}", "<Button", BUTTON_FLOOR],
    ["OfflineOverlay.tsx", "onClick={reconnect}", "<Button", BUTTON_FLOOR],
    ["ChunkBoundary.tsx", "onClick={() => location.reload()}", "<Button", BUTTON_FLOOR],
    ["terminal/partPieces.tsx", "plainBounce(props.ptyRef.current", "<Button", BUTTON_FLOOR],
    ["terminal/partPieces.tsx", "onClick={takeOver}", "<Button", ["max-md:min-h-11", "pointer-coarse:min-h-11"]],
    ["MacroPopover.tsx", "onClick={openDialog}", "<PopoverPrimitive.Close", BUTTON_FLOOR],
    ["FlatAgentList.tsx", "const HEADER_CONTROL_SIZING", "const", BUTTON_FLOOR],
    ["FlatAgentList.tsx", "aria-expanded={open}", "<button", BUTTON_FLOOR],
    ["FlatAgentList.tsx", "<Search className", "<div", BUTTON_FLOOR],
  ]
  for (const [file, marker, tag, classes] of cases) {
    it(`${file} ${marker}`, () => {
      const el = openingTag(read(file), marker, tag)
      for (const cls of classes) expect(el, `missing ${cls}`).toContain(cls)
    })
  }
})
