import { useLayoutEffect, useRef, useState } from "react"
import {
  AlertTriangle,
  Ban,
  FolderPlus,
} from "lucide-react"
import { notifyError } from "@/lib/notify"

import { GlyphSpinner } from "@/components/GlyphSpinner"
import { FolderBrowseList } from "@/components/FolderBrowseList"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import {
  addProjectPrimaryAction,
  branchWarningCopy,
  initRepoCopy,
  insideRepoCopy,
  noCommitsCopy,
} from "@/lib/addProjectWarning"
import type {
  AddProjectPrimaryAction,
  BranchWarningCopy,
  InitRepoCopy,
  InsideRepoCopy,
  NoCommitsCopy,
} from "@/lib/addProjectWarning"
import { browseApi } from "@/lib/browseApi"
import {
  addProject,
  addProjectCheckoutDefault,
  addProjectCreateInitialCommit,
  browseDir,
  closeAddProject,
  initProject,
  inspectProjectPath,
  useDux,
} from "@/lib/store"
import type { DuxState } from "@/lib/store"
import type { DirEntryView } from "@/lib/types"

// The inline "New folder" affordance in the dialog header: a ghost button that
// swaps for an input + Create/Cancel. On success it navigates INTO the new
// folder, where the pinned "Use this folder" row makes it the init target in
// one more tap. Errors surface the server's message as a toast.
function NewFolderControl({ browsePath }: { browsePath: string }) {
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState("")
  const [creating, setCreating] = useState(false)

  function reset() {
    setEditing(false)
    setName("")
    setCreating(false)
  }

  function create() {
    const trimmed = name.trim()
    if (!trimmed || creating) return
    setCreating(true)
    browseApi
      .mkdir(browsePath, trimmed)
      .then((created) => {
        reset()
        browseDir(created.path)
      })
      .catch((e) => {
        setCreating(false)
        notifyError(
          e instanceof Error ? e.message : "Could not create the folder.",
        )
      })
  }

  if (!editing) {
    return (
      <Button
        variant="ghost"
        size="sm"
        className="max-md:min-h-10 shrink-0"
        onClick={() => setEditing(true)}
      >
        <FolderPlus />
        New folder
      </Button>
    )
  }
  return (
    <div className="flex items-center gap-1">
      <Input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault()
            create()
          } else if (e.key === "Escape") {
            e.preventDefault()
            // stopPropagation is load-bearing: base-ui's dialog dismiss
            // listens for Escape at the document level and does NOT check
            // defaultPrevented, so without it the Escape meant to cancel
            // this inline editor also closes the whole picker (losing the
            // browse location, selection, and inspection).
            e.stopPropagation()
            reset()
          }
        }}
        placeholder="Folder name"
        className="h-8 max-md:min-h-10 w-40"
      />
      <Button
        variant="outline"
        size="sm"
        className="max-md:min-h-10"
        disabled={!name.trim() || creating}
        onClick={create}
      >
        Create
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="max-md:min-h-10"
        onClick={reset}
      >
        Cancel
      </Button>
    </div>
  )
}

// A read-only field for a full filesystem path. It is scrolled to the END
// whenever the value changes so the most specific (rightmost) segment stays
// visible by default, which is what matters when paths are long; the field
// remains scrollable back to the start and selectable so the path can be
// copied. Styling mirrors the app Input in a compact text-xs form. Used for
// both the current-directory header and the selected-target line so the two
// render identically.
function PathField({ value }: { value: string }) {
  const ref = useRef<HTMLInputElement>(null)
  // useLayoutEffect (not useEffect) so the scroll-to-end happens before paint:
  // otherwise the browser paints the leading segment for one frame and snaps to
  // the tail, which is exactly the flash this feature exists to avoid. Runs on
  // every value change so navigation re-reveals the tail. scrollWidth is 0 under
  // jsdom (no layout), so this is a harmless no-op in tests.
  useLayoutEffect(() => {
    const el = ref.current
    if (el) el.scrollLeft = el.scrollWidth
  }, [value])
  return (
    <input
      ref={ref}
      readOnly
      // tabIndex=-1 keeps this out of the tab order and the dialog's initial
      // focus (it is an inert, read-only field), while staying click-selectable
      // so the path can still be copied.
      tabIndex={-1}
      value={value}
      aria-label="Path"
      className="w-full min-w-0 rounded-lg border border-input bg-input/30 px-2 py-1 font-mono text-xs text-muted-foreground outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
    />
  )
}

interface InspectionMessagesProps {
  init: InitRepoCopy | null
  blocked: InsideRepoCopy | null
  noCommits: NoCommitsCopy | null
  branch: BranchWarningCopy | null
  checkoutDefault: boolean
  setCheckoutDefault: (checked: boolean) => void
}

function InspectionMessages({
  init,
  blocked,
  noCommits,
  branch,
  checkoutDefault,
  setCheckoutDefault,
}: InspectionMessagesProps) {
  return (
    <>
      {init ? (
        <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />
            <div className="grid gap-1 text-sm">
              <span>{init.message}</span>
              <span className="text-xs text-muted-foreground">{init.note}</span>
            </div>
          </div>
        </div>
      ) : null}
      {blocked ? (
        <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
          <div className="flex items-start gap-2">
            <Ban className="mt-0.5 size-4 shrink-0 text-amber-500" />
            <span className="text-sm">{blocked.message}</span>
          </div>
        </div>
      ) : null}
      {noCommits ? (
        <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />
            <div className="grid gap-1 text-sm">
              <span>{noCommits.message}</span>
              <span className="text-xs text-muted-foreground">
                {noCommits.note}
              </span>
            </div>
          </div>
        </div>
      ) : null}
      {branch ? (
        <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />
            <div className="grid gap-1 text-sm">
              <span>{branch.message}</span>
              <span className="text-amber-500">{branch.worktreeNote}</span>
              {branch.heuristicNote ? (
                <span className="text-xs text-muted-foreground">
                  {branch.heuristicNote}
                </span>
              ) : null}
            </div>
          </div>
          {branch.canCheckoutDefault && branch.defaultBranch ? (
            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                checked={checkoutDefault}
                onCheckedChange={(checked) =>
                  setCheckoutDefault(checked === true)
                }
              />
              Check out &ldquo;{branch.defaultBranch}&rdquo; before adding
            </label>
          ) : null}
        </div>
      ) : null}
    </>
  )
}

interface SelectionDetailsProps extends InspectionMessagesProps {
  selected: string
  name: string
  inspecting: boolean
  setName: (name: string) => void
}

function SelectionDetails({
  selected,
  name,
  inspecting,
  setName,
  ...messages
}: SelectionDetailsProps) {
  return (
    <div className="flex flex-col gap-2">
      <Input
        value={name}
        onChange={(event) => setName(event.target.value)}
        placeholder="Project name (optional)"
      />
      <PathField value={selected} />
      {inspecting ? (
        <span className="flex items-center gap-2 text-xs text-muted-foreground">
          <GlyphSpinner className="text-muted-foreground" />
          Checking the folder…
        </span>
      ) : null}
      <InspectionMessages {...messages} />
    </div>
  )
}

interface AddProjectFooterProps {
  selected: string | null
  inspecting: boolean
  inspected: boolean
  primary: AddProjectPrimaryAction
  hasBranchWarning: boolean
  onAdd: () => void
}

function AddProjectFooter({
  selected,
  inspecting,
  inspected,
  primary,
  hasBranchWarning,
  onAdd,
}: AddProjectFooterProps) {
  return (
    <DialogFooter>
      <Button variant="outline" onClick={closeAddProject}>
        Cancel
      </Button>
      <Button
        disabled={
          !selected || inspecting || !inspected || primary.action === "blocked"
        }
        variant={hasBranchWarning ? "destructive" : "default"}
        onClick={onAdd}
      >
        {primary.label}
      </Button>
    </DialogFooter>
  )
}

type ProjectInspection = NonNullable<DuxState["projectPathInspection"]>

function currentInspection(
  selected: string | null,
  candidate: DuxState["projectPathInspection"],
): ProjectInspection | null {
  if (!selected || candidate?.path !== selected) return null
  return candidate
}

function inspectionKind(inspection: ProjectInspection | null) {
  if (!inspection || inspection.error) return "repo" as const
  return inspection.kind
}

function requiresInitialCommit(
  inspection: ProjectInspection | null,
): boolean {
  if (
    !inspection ||
    inspection.error ||
    inspection.loading ||
    inspection.hasCommits
  ) {
    return false
  }
  return inspection.kind !== "plain" && inspection.kind !== "repo_subdir"
}

function inspectionBranch(
  inspection: ProjectInspection | null,
  needsInitialCommit: boolean,
): BranchWarningCopy | null {
  if (
    needsInitialCommit ||
    !inspection ||
    inspection.error ||
    !inspection.warning ||
    !inspection.currentBranch
  ) {
    return null
  }
  return branchWarningCopy(inspection.warning, inspection.currentBranch)
}

function inspectionState(
  selected: string | null,
  candidate: DuxState["projectPathInspection"],
  checkoutDefault: boolean,
) {
  const inspection = currentInspection(selected, candidate)
  const kind = inspectionKind(inspection)
  const needsInitialCommit = requiresInitialCommit(inspection)
  const branch = inspectionBranch(inspection, needsInitialCommit)
  const willCheckout = Boolean(branch?.canCheckoutDefault && checkoutDefault)
  const primary = addProjectPrimaryAction({
    kind,
    hasCommits: !needsInitialCommit,
    willCheckout,
    hasBranchWarning: Boolean(branch),
  })
  const resolved = Boolean(inspection && !inspection.loading)

  return {
    inspection,
    inspecting: inspection?.loading ?? false,
    primary,
    branch,
    init:
      resolved && primary.action === "init-repo"
        ? initRepoCopy(inspection?.gitignoreCandidates ?? [])
        : null,
    blocked:
      resolved && primary.action === "blocked"
        ? insideRepoCopy(inspection?.repoRoot ?? null)
        : null,
    noCommits: needsInitialCommit ? noCommitsCopy() : null,
  }
}

// A monospace name chip with a leading folder glyph. Shared by the pinned
// "Use this folder" row and the "Up to <folder>" parent row so the two folder
// names render identically and cannot drift.
// The browser body is mounted only while the dialog is open so its local
// `selected`/`name` state resets on each open — no set-state-in-effect needed.
function AddProjectBrowser() {
  const {
    browsePath,
    browseEntries,
    browseLoading,
    projectPathInspection,
    addProjectIntent,
  } = useDux()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")
  // Whether to check out the default branch before adding, mirroring the TUI's
  // "Check Out & Add" checkbox. Defaulted ON when a Known warning is shown (the
  // TUI defaults it on too); ignored on the heuristic path (no checkbox there).
  const [checkoutDefault, setCheckoutDefault] = useState(true)

  const {
    inspection,
    inspecting,
    primary,
    branch,
    init,
    blocked,
    noCommits,
  } = inspectionState(selected, projectPathInspection, checkoutDefault)

  function selectTarget(path: string) {
    setSelected(path)
    setCheckoutDefault(true)
    // Inspect the target so the panel/warning step reflects the server's
    // classification before adding (mirrors the TUI's add_project pre-flight).
    inspectProjectPath(path)
  }

  function handleEntryClick(entry: DirEntryView) {
    // Every folder row opens on click, git repo or not, matching the TUI's
    // navigate-anywhere model: a git repo is not a dead end, you can browse into
    // it and its subfolders. Choosing a target is done exclusively through the
    // pinned "Use this folder" row (which inspects and, for a repo subdirectory,
    // surfaces the same "add the repository instead" block as the TUI). This
    // also clears any pending selection from the directory we are leaving.
    setSelected(null)
    browseDir(entry.path)
  }

  function handleAdd() {
    if (!selected) return
    if (primary.action === "blocked") return
    if (primary.action === "init-repo") {
      initProject(selected, name)
    } else if (primary.action === "initial-commit") {
      addProjectCreateInitialCommit(selected, name)
    } else if (primary.action === "checkout-default") {
      addProjectCheckoutDefault(selected, name)
    } else {
      addProject(selected, name)
    }
    setSelected(null)
    setName("")
    closeAddProject()
  }

  const usingThisFolder = selected === browsePath

  return (
    <DialogContent className="sm:max-w-xl" showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Add a project</DialogTitle>
        {addProjectIntent === "init" ? (
          <span className="text-xs text-muted-foreground">
            Pick or create a folder, then choose Use this folder; dux will
            initialize a git repository in it.
          </span>
        ) : null}
        <div className="flex items-center justify-between gap-2">
          <PathField value={browsePath} />
          <NewFolderControl browsePath={browsePath} />
        </div>
      </DialogHeader>

      <FolderBrowseList
        path={browsePath}
        entries={browseEntries}
        loading={browseLoading}
        commitLabel="Use this folder"
        committed={usingThisFolder}
        onCommit={selectTarget}
        onOpen={handleEntryClick}
      />

      {selected ? (
        <SelectionDetails
          selected={selected}
          name={name}
          inspecting={inspecting}
          setName={setName}
          init={init}
          blocked={blocked}
          noCommits={noCommits}
          branch={branch}
          checkoutDefault={checkoutDefault}
          setCheckoutDefault={setCheckoutDefault}
        />
      ) : null}

      <AddProjectFooter
        selected={selected}
        inspecting={inspecting}
        inspected={!!inspection}
        primary={primary}
        hasBranchWarning={!!branch}
        onAdd={handleAdd}
      />
    </DialogContent>
  )
}

export function AddProjectDialog() {
  const { addProjectOpen } = useDux()

  return (
    <Dialog
      open={addProjectOpen}
      onOpenChange={(o) => {
        if (!o) closeAddProject()
      }}
    >
      {addProjectOpen && <AddProjectBrowser />}
    </Dialog>
  )
}
