import { useState } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { closeCreateAgent, createAgent, useDux } from "@/lib/store"

// The form body is mounted only while the dialog is open. It seeds its
// `useState` via a lazy initializer so there is no set-state-in-effect to seed
// the input on open.
function CreateAgentForm({
  projectId,
  projectName,
}: {
  projectId: string
  projectName: string
}) {
  const [name, setName] = useState(() => "")

  function handleCreate() {
    // An empty name is valid: the server auto-generates a branch name.
    createAgent(projectId, name.trim())
    closeCreateAgent()
  }

  return (
    <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>New agent in {projectName}</DialogTitle>
        <DialogDescription>
          Creates a git worktree + branch and launches the agent. Leave the name
          blank to auto-generate a branch name.
        </DialogDescription>
      </DialogHeader>
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault()
            handleCreate()
          }
        }}
        placeholder="Branch name (optional)"
        autoFocus
      />
      <DialogFooter>
        <Button variant="outline" onClick={closeCreateAgent}>
          Cancel
        </Button>
        <Button onClick={handleCreate}>Create agent</Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function CreateAgentDialog() {
  const { createAgentTarget, viewModel } = useDux()
  const open = createAgentTarget !== null
  const project = viewModel?.projects.find((p) => p.id === createAgentTarget)

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeCreateAgent()
      }}
    >
      {open && createAgentTarget && (
        <CreateAgentForm
          projectId={createAgentTarget}
          projectName={project?.name ?? "project"}
        />
      )}
    </Dialog>
  )
}
