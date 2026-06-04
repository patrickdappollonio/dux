import { useState } from "react"
import { Folder, FolderGit2 } from "lucide-react"

import { BrailleSpinner } from "@/components/BrailleSpinner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { addProject, browseDir, closeAddProject, useDux } from "@/lib/store"
import type { DirEntryView } from "@/lib/types"

// The browser body is mounted only while the dialog is open so its local
// `selected`/`name` state resets on each open — no set-state-in-effect needed.
function AddProjectBrowser() {
  const { browsePath, browseEntries, browseLoading } = useDux()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")

  function handleEntryClick(entry: DirEntryView) {
    if (entry.is_git_repo) {
      setSelected(entry.path)
    } else {
      // Navigating away clears any pending selection from the prior directory.
      setSelected(null)
      browseDir(entry.path)
    }
  }

  function handleAdd() {
    if (!selected) return
    addProject(selected, name)
    setSelected(null)
    setName("")
    closeAddProject()
  }

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
        </div>
      ) : null}

      <DialogFooter>
        <Button variant="outline" onClick={closeAddProject}>
          Cancel
        </Button>
        <Button disabled={!selected} onClick={handleAdd}>
          Add project
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
