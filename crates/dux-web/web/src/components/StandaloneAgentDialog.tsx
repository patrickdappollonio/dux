import { useState } from "react"

import { FolderBrowseList } from "@/components/FolderBrowseList"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { baseName } from "@/lib/paths"
import {
  browseDir,
  closeStandaloneAgentPicker,
  createStandaloneAgent,
  useDux,
} from "@/lib/store"

/**
 * Pick a folder you already have and run an agent in it.
 *
 * The browsing is the add-project picker's, through the shared
 * `FolderBrowseList` and the store's one browse slice. What is deliberately
 * ABSENT is that picker's inspection ladder: a project must be a repository, so
 * adding one classifies the folder first and offers to `git init` a plain one.
 * A standalone agent accepts whatever is there, initializes nothing, and never
 * modifies the folder, so there is nothing to check and nothing to warn about.
 *
 * Every refusal (a relative path, a folder that already hosts a standalone
 * agent) is the SERVER's and arrives as a toast, shared with the terminal UI so
 * the two surfaces cannot answer the same question differently.
 */
function StandaloneAgentBrowser() {
  const { browsePath, browseEntries, browseLoading } = useDux()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")

  function handleCreate() {
    if (!selected) return
    createStandaloneAgent(selected, name)
    setSelected(null)
    setName("")
    closeStandaloneAgentPicker()
  }

  return (
    <DialogContent className="sm:max-w-xl" showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Run an agent in a folder</DialogTitle>
        <span className="text-xs text-muted-foreground">
          Pick any folder. The agent runs there directly: no branch, no
          worktree, and dux never creates, moves or removes the folder.
        </span>
      </DialogHeader>

      <FolderBrowseList
        path={browsePath}
        entries={browseEntries}
        loading={browseLoading}
        commitLabel="Run an agent here"
        committed={selected === browsePath}
        onCommit={setSelected}
        onOpen={(entry) => {
          // Navigating away abandons the pending choice, so the footer can
          // never act on a folder the user has left.
          setSelected(null)
          browseDir(entry.path)
        }}
      />

      {selected ? (
        <div className="flex flex-col gap-2">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            // Empty is the ordinary case: the server names the agent after the
            // folder. A typed name is used as typed, since no branch is created
            // and no ref-name rule applies.
            placeholder={`Agent name (optional, defaults to "${baseName(selected)}")`}
          />
          <span className="font-mono text-xs break-all text-muted-foreground">
            {selected}
          </span>
        </div>
      ) : null}

      <DialogFooter>
        <Button variant="outline" onClick={closeStandaloneAgentPicker}>
          Cancel
        </Button>
        <Button onClick={handleCreate} disabled={!selected}>
          Create agent
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function StandaloneAgentDialog() {
  const { standaloneAgentPickerOpen } = useDux()
  return (
    <Dialog
      open={standaloneAgentPickerOpen}
      onOpenChange={(open) => {
        if (!open) closeStandaloneAgentPicker()
      }}
    >
      {/* Mounted only while open, so the browse slice is loaded fresh on every
          open rather than showing wherever the last pick left off. */}
      {standaloneAgentPickerOpen && <StandaloneAgentBrowser />}
    </Dialog>
  )
}
