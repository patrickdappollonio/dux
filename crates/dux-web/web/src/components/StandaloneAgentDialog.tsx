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
import { standaloneAgentDefaultName } from "@/lib/paths"
import {
  browseDir,
  closeStandaloneAgentPicker,
  createStandaloneAgent,
  useDux,
} from "@/lib/store"

/**
 * Pick a folder you already have and run an agent in it.
 *
 * The browsing is the add-project picker's, through the shared `FolderBrowseList`
 * and the store's one browse slice. That picker's inspection ladder is
 * deliberately absent: a standalone agent accepts whatever is there, initializes
 * nothing and never modifies the folder, so there is nothing to check.
 *
 * Every refusal is the server's and arrives as a toast, shared with the terminal
 * UI so the two surfaces cannot answer the same question differently.
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
        <DialogTitle>New standalone agent</DialogTitle>
        {/* The words "standalone agent" appear here on purpose: every other
          * surface uses that name, so the dialog that creates one teaches it. */}
        <span className="text-xs text-muted-foreground">
          Pick any folder and a standalone agent runs there directly: no branch,
          no worktree, and dux never creates, moves or removes the folder.
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
            // folder. A typed name is used verbatim, no branch being created and
            // no ref-name rule applying. The placeholder is derived through the
            // twin of the server's sanitizer, so it cannot promise another name.
            placeholder={`Agent name (optional, defaults to "${standaloneAgentDefaultName(selected)}")`}
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
