import { useState } from "react"
import {
  AlertTriangle,
  Ban,
  Folder,
  FolderGit2,
  FolderOpen,
  FolderPlus,
} from "lucide-react"
import { toast } from "sonner"

import { BrailleSpinner } from "@/components/BrailleSpinner"
import { Badge } from "@/components/ui/badge"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  addProjectPrimaryAction,
  branchWarningCopy,
  initRepoCopy,
  insideRepoCopy,
  noCommitsCopy,
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
import type { DirEntryView } from "@/lib/types"

// Basename of a browse path for the pinned row's muted suffix.
function baseName(path: string): string {
  const trimmed = path.endsWith("/") && path !== "/" ? path.slice(0, -1) : path
  const idx = trimmed.lastIndexOf("/")
  return idx >= 0 ? trimmed.slice(idx + 1) || trimmed : trimmed
}

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
        toast.error(
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

  // The resolved inspection for the CURRENT selection only. A reply for a stale
  // path is already discarded in the store, but guard here too so the warning
  // step never renders for a different target than the one selected. This also
  // covers the pinned "Use this folder" row, whose target is `browsePath`.
  const inspection =
    selected && projectPathInspection?.path === selected
      ? projectPathInspection
      : null
  const inspecting = inspection?.loading ?? false
  const kind = inspection && !inspection.error ? inspection.kind : "repo"
  // A resolved inspection of an unborn repo (fresh `git init`, no commits)
  // takes precedence over any branch warning: there is no default branch to
  // check out, and after the initial commit the current branch simply becomes
  // the leading branch. We only offer it once inspection confirms it.
  const needsInitialCommit =
    !!inspection &&
    !inspection.error &&
    !inspection.loading &&
    !inspection.hasCommits &&
    kind !== "plain" &&
    kind !== "repo_subdir"
  const warning = inspection && !inspection.error ? inspection.warning : null
  const copy =
    !needsInitialCommit && warning && inspection?.currentBranch
      ? branchWarningCopy(warning, inspection.currentBranch)
      : null
  // Only offer the checkbox when the server confidently knows the default.
  const offerCheckout = copy?.canCheckoutDefault ?? false
  const willCheckout = offerCheckout && checkoutDefault
  const noCommits = needsInitialCommit ? noCommitsCopy() : null
  const primary = addProjectPrimaryAction({
    kind,
    hasCommits: !needsInitialCommit,
    willCheckout,
    hasBranchWarning: !!copy,
  })
  const resolved = !!inspection && !inspection.loading
  const initCopy =
    resolved && primary.action === "init-repo"
      ? initRepoCopy(inspection?.gitignoreCandidates ?? [])
      : null
  const blockedCopy =
    resolved && primary.action === "blocked"
      ? insideRepoCopy(inspection?.repoRoot ?? null)
      : null

  function selectTarget(path: string) {
    setSelected(path)
    setCheckoutDefault(true)
    // Inspect the target so the panel/warning step reflects the server's
    // classification before adding (mirrors the TUI's add_project pre-flight).
    inspectProjectPath(path)
  }

  function handleEntryClick(entry: DirEntryView) {
    if (entry.is_git_repo) {
      selectTarget(entry.path)
    } else {
      // Navigating away clears any pending selection from the prior directory.
      setSelected(null)
      browseDir(entry.path)
    }
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

  const confirmLabel = primary.label
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
          <span className="font-mono text-xs text-muted-foreground truncate">
            {browsePath}
          </span>
          <NewFolderControl browsePath={browsePath} />
        </div>
      </DialogHeader>

      <ScrollArea className="h-[50vh] rounded-md border md:h-80">
        {browseLoading ? (
          <div className="flex h-[50vh] items-center justify-center md:h-80">
            <BrailleSpinner className="text-lg text-muted-foreground" />
          </div>
        ) : (
          <div className="flex flex-col">
            {/* Pinned, client-synthesized "Use this folder" row: the ONLY way
                the current directory becomes a target. The footer stays
                strictly selection-driven; the primary button never acts on
                wherever the user happens to be standing. */}
            <button
              type="button"
              onClick={() => selectTarget(browsePath)}
              className={`flex min-h-11 items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent md:min-h-0 ${
                usingThisFolder ? "bg-accent" : ""
              }`}
            >
              <FolderOpen className="size-4 shrink-0 text-muted-foreground" />
              <span className="flex-1 truncate">
                Use this folder
                <span className="ml-2 text-muted-foreground">
                  {baseName(browsePath)}
                </span>
              </span>
            </button>
            {browseEntries.map((entry) => {
              const isSelected = entry.is_git_repo && selected === entry.path
              const Icon = entry.is_git_repo ? FolderGit2 : Folder
              return (
                <button
                  key={entry.path}
                  type="button"
                  onClick={() => handleEntryClick(entry)}
                  // min-h-11 on phones gives each row a ≥44px touch target;
                  // desktop keeps the compact py-2 density via md:.
                  className={`flex min-h-11 items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent md:min-h-0 ${
                    isSelected ? "bg-accent" : ""
                  }`}
                >
                  <Icon className="size-4 shrink-0 text-muted-foreground" />
                  <span className="flex-1 truncate">{entry.label}</span>
                  {entry.is_git_repo ? (
                    <Badge variant="secondary" className="shrink-0">
                      git
                    </Badge>
                  ) : null}
                </button>
              )
            })}
          </div>
        )}
      </ScrollArea>

      {selected ? (
        <div className="grid gap-2">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Project name (optional)"
          />
          <span className="font-mono text-xs text-muted-foreground truncate">
            {selected}
          </span>
          {inspecting ? (
            <span className="flex items-center gap-2 text-xs text-muted-foreground">
              <BrailleSpinner className="text-muted-foreground" />
              Checking the folder…
            </span>
          ) : null}
          {initCopy ? (
            /* Init panel: this panel plus the explicit "Initialize Repository
               & Add" label IS the confirmation; no extra dialog, because the
               action is non-destructive (append-only no-follow seed, empty
               commit, git init in a folder the server confirmed is not a
               repo), consistent with the initial-commit rung's shipped
               confirm-by-labeled-button. */
            <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
              <div className="flex items-start gap-2">
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />
                <div className="grid gap-1 text-sm">
                  <span>{initCopy.message}</span>
                  <span className="text-xs text-muted-foreground">
                    {initCopy.note}
                  </span>
                </div>
              </div>
            </div>
          ) : null}
          {blockedCopy ? (
            <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
              <div className="flex items-start gap-2">
                <Ban className="mt-0.5 size-4 shrink-0 text-amber-500" />
                <span className="text-sm">{blockedCopy.message}</span>
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
          {copy ? (
            <div className="grid gap-2 rounded-md border border-amber-600/40 bg-amber-600/10 p-3">
              <div className="flex items-start gap-2">
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />
                <div className="grid gap-1 text-sm">
                  <span>{copy.message}</span>
                  <span className="text-amber-500">{copy.worktreeNote}</span>
                  {copy.heuristicNote ? (
                    <span className="text-xs text-muted-foreground">
                      {copy.heuristicNote}
                    </span>
                  ) : null}
                </div>
              </div>
              {offerCheckout && copy.defaultBranch ? (
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={checkoutDefault}
                    onCheckedChange={(c) => setCheckoutDefault(c === true)}
                  />
                  Check out &ldquo;{copy.defaultBranch}&rdquo; before adding
                </label>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      <DialogFooter>
        <Button variant="outline" onClick={closeAddProject}>
          Cancel
        </Button>
        <Button
          disabled={
            !selected ||
            inspecting ||
            !inspection ||
            primary.action === "blocked"
          }
          variant={copy ? "destructive" : "default"}
          onClick={handleAdd}
        >
          {confirmLabel}
        </Button>
      </DialogFooter>
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
