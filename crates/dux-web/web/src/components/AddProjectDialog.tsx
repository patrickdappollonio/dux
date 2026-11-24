import { useLayoutEffect, useRef, useState, type Ref } from "react"
import {
  AlertTriangle,
  Ban,
  FolderPlus,
} from "lucide-react"
import { Notice } from "@/components/Notice"
import { notifyError } from "@/lib/notify"

import { LoadingSentence } from "@/components/Spinner"
import { FolderBrowseList } from "@/components/FolderBrowseList"
import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { Button } from "@/components/ui/button"
import { CheckboxRow } from "@/components/CheckboxRow"
import { InlineCode } from "@/components/ui/inline-code"
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
import { renderProse } from "@/lib/prose"
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
import { cn } from "@/lib/utils"

// The inline "New folder" affordance beside the current path: a ghost button that
// swaps for an input + Create/Cancel. On success it navigates INTO the new
// folder, where the pinned "Use this folder" row makes it the init target in
// one more tap. Errors surface the server's message as a toast.
function NewFolderControl({
  browsePath,
  buttonRef,
}: {
  browsePath: string
  buttonRef: Ref<HTMLButtonElement>
}) {
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
        void browseDir(created.path)
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
        ref={buttonRef}
        variant="ghost"
        size="sm"
        className="max-md:min-h-10 pointer-coarse:min-h-10 shrink-0"
        onClick={() => setEditing(true)}
      >
        <FolderPlus />
        New folder
      </Button>
    )
  }
  return (
    // Beside the path on a computer; on a phone a row of its own, where the
    // name takes the width the two buttons leave.
    <div className="flex min-w-0 items-center gap-1 max-md:w-full">
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
        className="h-8 w-40 min-w-0 flex-1 max-md:h-10 pointer-coarse:h-10"
      />
      <Button
        variant="outline"
        size="sm"
        className="max-md:min-h-10 pointer-coarse:min-h-10"
        disabled={!name.trim() || creating}
        onClick={create}
      >
        Create
      </Button>
      <Button
        variant="ghost"
        size="sm"
        className="max-md:min-h-10 pointer-coarse:min-h-10"
        onClick={reset}
      >
        Cancel
      </Button>
    </div>
  )
}

// A read-only field for a full filesystem path, scrolled to the end on every
// change so the rightmost segment stays visible, and still scrollable and
// selectable so the path can be copied. Shared by the current-directory header
// and the selected-target line so the two render identically.
// "row" shares a wrapping row with the New folder control; "column" sits in a
// column, where a flex basis would be its height.
function PathField({ value, place }: { value: string; place: "row" | "column" }) {
  const ref = useRef<HTMLInputElement>(null)
  // useLayoutEffect so the scroll-to-end happens before paint: otherwise the
  // browser paints the leading segment for one frame and snaps to the tail.
  // scrollWidth is 0 under jsdom, so this is a no-op in tests.
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
      className={cn(
        "w-full min-w-0 rounded-lg border border-input bg-input/30 px-2 py-1 font-mono text-xs text-muted-foreground outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
        place === "row" && "flex-1 basis-56",
      )}
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
        <Notice shape="box" tone="warning" announce="static" icon={AlertTriangle}>
          <div className="grid gap-1">
            <span>{init.message}</span>
            <span className="text-xs text-muted-foreground">
              {renderProse(init.noteProse)}
            </span>
          </div>
        </Notice>
      ) : null}
      {blocked ? (
        <Notice shape="box" tone="warning" announce="static" icon={Ban}>
          {renderProse(blocked.messageProse)}
        </Notice>
      ) : null}
      {noCommits ? (
        <Notice shape="box" tone="warning" announce="static" icon={AlertTriangle}>
          <div className="grid gap-1">
            <span>{noCommits.message}</span>
            <span className="text-xs text-muted-foreground">
              {noCommits.note}
            </span>
          </div>
        </Notice>
      ) : null}
      {branch ? (
        <Notice
          shape="box"
          tone="warning"
          announce="static"
          icon={AlertTriangle}
          below={
            branch.canCheckoutDefault && branch.defaultBranch ? (
              <CheckboxRow
                id="add-project-checkout-default"
                checked={checkoutDefault}
                onCheckedChange={setCheckoutDefault}
              >
                Check out <InlineCode>{branch.defaultBranch}</InlineCode> before
                adding
              </CheckboxRow>
            ) : null
          }
        >
          <div className="grid gap-1">
            <span>{renderProse(branch.messageProse)}</span>
            {/* Warning tone only when worktrees will NOT branch from the
                default; the same tone as the panel's own warning glyph. */}
            <span
              data-tone={branch.worktreeTone}
              className={
                branch.worktreeTone === "warning"
                  ? "text-dux-warning"
                  : "text-muted-foreground"
              }
            >
              {renderProse(branch.worktreeNoteProse)}
            </span>
            {branch.heuristicNote ? (
              <span className="text-xs text-muted-foreground">
                {branch.heuristicNote}
              </span>
            ) : null}
          </div>
        </Notice>
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
    <div className="flex shrink-0 flex-col gap-2">
      <Input
        value={name}
        onChange={(event) => setName(event.target.value)}
        placeholder="Project name (optional)"
      />
      <PathField value={selected} place="column" />
      {inspecting ? (
        <LoadingSentence spinner="glyph" size="xs">
          Checking the folder…
        </LoadingSentence>
      ) : null}
      <InspectionMessages {...messages} />
    </div>
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
  checkoutDefault: boolean,
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
  return branchWarningCopy(
    inspection.warning,
    inspection.currentBranch,
    checkoutDefault,
  )
}

// The explanatory panel under the picker, one field per panel and at most one of
// them set. Each waits for the inspection to resolve, so a folder mid-inspection
// explains nothing rather than explaining the last folder.
function inspectionPanels(
  inspection: ProjectInspection | null,
  primaryAction: string,
  needsInitialCommit: boolean,
) {
  const noCommits = needsInitialCommit ? noCommitsCopy() : null
  if (!inspection || inspection.loading) {
    return { init: null, blocked: null, noCommits }
  }
  return {
    init:
      primaryAction === "init-repo"
        ? initRepoCopy(inspection.gitignoreCandidates ?? [])
        : null,
    blocked:
      primaryAction === "blocked"
        ? insideRepoCopy(inspection.repoRoot ?? null)
        : null,
    noCommits,
  }
}

function inspectionState(
  selected: string | null,
  candidate: DuxState["projectPathInspection"],
  checkoutDefault: boolean,
) {
  const inspection = currentInspection(selected, candidate)
  const kind = inspectionKind(inspection)
  const needsInitialCommit = requiresInitialCommit(inspection)
  const branch = inspectionBranch(inspection, needsInitialCommit, checkoutDefault)
  const willCheckout = Boolean(branch?.canCheckoutDefault && checkoutDefault)
  const primary = addProjectPrimaryAction({
    kind,
    hasCommits: !needsInitialCommit,
    willCheckout,
    hasBranchWarning: Boolean(branch),
  })
  return {
    inspection,
    inspecting: inspection?.loading ?? false,
    primary,
    branch,
    ...inspectionPanels(inspection, primary.action, needsInitialCommit),
  }
}

// Why the primary button takes no press, or null when it may be pressed.
function addRefusal(
  selected: string | null,
  inspecting: boolean,
  inspected: boolean,
  action: AddProjectPrimaryAction["action"],
): string | null {
  if (!selected) return "Choose a folder with Use this folder first."
  if (inspecting || !inspected) return "dux is still checking the folder."
  if (action === "blocked") return "This folder is inside a git repository. Add that repository instead."
  return null
}

function AddProjectBrowser({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const state = useDux()
  const { browsePath, browseEntries, browseLoading } = state
  // Closing clears both in the store; held, the hint and the folder's verdict
  // stay as they were while the close animates.
  const projectPathInspection = useHeldWhileClosed(open, state.projectPathInspection)
  const addProjectIntent = useHeldWhileClosed(open, state.addProjectIntent)
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")
  // Whether to check out the default branch before adding, mirroring the TUI's
  // "Check Out & Add" checkbox. Defaulted ON when a Known warning is shown (the
  // TUI defaults it on too); ignored on the heuristic path (no checkbox there).
  const [checkoutDefault, setCheckoutDefault] = useState(true)
  const newFolderRef = useRef<HTMLButtonElement | null>(null)

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
    void browseDir(entry.path)
  }

  function handleAdd() {
    if (!selected) return
    if (primary.action === "blocked") return
    // The selection and the name stay as they are, so the closing dialog
    // shows what was added; the next open starts empty.
    if (primary.action === "init-repo") {
      initProject(selected, name)
    } else if (primary.action === "initial-commit") {
      addProjectCreateInitialCommit(selected, name)
    } else if (primary.action === "checkout-default") {
      addProjectCheckoutDefault(selected, name)
    } else {
      addProject(selected, name)
    }
    closeAddProject()
  }

  return (
    <Modal
      kind="form"
      open={open}
      onClose={closeAddProject}
      onClosed={onClosed}
      size="xl"
      title="Add a project"
      description={
        addProjectIntent === "init"
          ? "Pick or create a folder, then choose Use this folder; dux will initialize a git repository in it."
          : undefined
      }
      // The first control, as before the folder list has anything to offer.
      initialFocus={newFolderRef}
      // Where the browse stands, and the way to make a folder there. The two
      // share a row while both fit and stack when they do not.
      toolbar={
        <div className="flex flex-wrap items-center justify-between gap-2">
          <PathField value={browsePath} place="row" />
          <NewFolderControl browsePath={browsePath} buttonRef={newFolderRef} />
        </div>
      }
      submit={{
        label: primary.label,
        tone: branch ? "destructive" : "primary",
        onSelect: handleAdd,
        disabledReason: addRefusal(selected, inspecting, !!inspection, primary.action),
      }}
    >
      <FolderBrowseList
        shrinkable
        path={browsePath}
        entries={browseEntries}
        loading={browseLoading}
        commitLabel="Use this folder"
        committed={selected === browsePath}
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
    </Modal>
  )
}

export function AddProjectDialog() {
  const { addProjectOpen } = useDux()
  const life = useModalLifetime(addProjectOpen)
  // Keyed by the open, so the selection and the name start empty on every one.
  return life.mounted ? (
    <AddProjectBrowser key={life.epoch} open={addProjectOpen} onClosed={life.onClosed} />
  ) : null
}
