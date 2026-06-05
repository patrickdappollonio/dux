import { useState } from "react"
import { AlertTriangle, Folder, FolderGit2 } from "lucide-react"

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
import { addConfirmLabel, branchWarningCopy } from "@/lib/addProjectWarning"
import {
  addProject,
  addProjectCheckoutDefault,
  browseDir,
  closeAddProject,
  inspectProjectPath,
  useDux,
} from "@/lib/store"
import type { DirEntryView } from "@/lib/types"

// The browser body is mounted only while the dialog is open so its local
// `selected`/`name` state resets on each open — no set-state-in-effect needed.
function AddProjectBrowser() {
  const { browsePath, browseEntries, browseLoading, projectPathInspection } =
    useDux()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")
  // Whether to check out the default branch before adding, mirroring the TUI's
  // "Check Out & Add" checkbox. Defaulted ON when a Known warning is shown (the
  // TUI defaults it on too); ignored on the heuristic path (no checkbox there).
  const [checkoutDefault, setCheckoutDefault] = useState(true)

  // The resolved inspection for the CURRENT selection only. A reply for a stale
  // path is already discarded in the store, but guard here too so the warning
  // step never renders for a different repo than the one selected.
  const inspection =
    selected && projectPathInspection?.path === selected
      ? projectPathInspection
      : null
  const warning = inspection && !inspection.error ? inspection.warning : null
  const copy =
    warning && inspection?.currentBranch
      ? branchWarningCopy(warning, inspection.currentBranch)
      : null
  // Only offer the checkbox when the server confidently knows the default.
  const offerCheckout = copy?.canCheckoutDefault ?? false
  const willCheckout = offerCheckout && checkoutDefault

  function handleEntryClick(entry: DirEntryView) {
    if (entry.is_git_repo) {
      setSelected(entry.path)
      setCheckoutDefault(true)
      // Inspect the repo's branch so a non-default branch surfaces the warning
      // step before adding (mirrors the TUI's add_project pre-flight).
      inspectProjectPath(entry.path)
    } else {
      // Navigating away clears any pending selection from the prior directory.
      setSelected(null)
      browseDir(entry.path)
    }
  }

  function handleAdd() {
    if (!selected) return
    if (willCheckout) {
      addProjectCheckoutDefault(selected, name)
    } else {
      addProject(selected, name)
    }
    setSelected(null)
    setName("")
    closeAddProject()
  }

  const inspecting = inspection?.loading ?? false
  const confirmLabel = copy ? addConfirmLabel(willCheckout) : "Add project"

  return (
    <DialogContent className="sm:max-w-xl" showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Add a project</DialogTitle>
        <span className="font-mono text-xs text-muted-foreground truncate">
          {browsePath}
        </span>
      </DialogHeader>

      <ScrollArea className="h-[50vh] rounded-md border md:h-80">
        {browseLoading ? (
          <div className="flex h-[50vh] items-center justify-center md:h-80">
            <BrailleSpinner className="text-lg text-muted-foreground" />
          </div>
        ) : (
          <div className="flex flex-col">
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
              Checking the current branch…
            </span>
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
          disabled={!selected || inspecting}
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
