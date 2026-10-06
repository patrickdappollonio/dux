import type { KeyboardEvent } from "react"

import {
  AgentNameInput,
  RandomizeNameCheckbox,
} from "@/components/AgentNameFields"
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
import { isValidAgentName } from "@/lib/agentName"
import {
  closeCloneProject,
  setCloneProjectName,
  setCloneProjectPath,
  setCloneProjectUrl,
  submitCloneProject,
  toggleCloneProjectRandomize,
  useDux,
} from "@/lib/store"

// Clone a repository as a new project and start its first agent. The server
// answers a refused check with its sentence, which stays on screen with the
// dialog open; a started clone closes it, and the progress and the one final
// ride the status toasts from there.
export function CloneProjectDialog() {
  const { cloneProject: form } = useDux()
  if (!form) return null
  const name = form.name.trim()
  const invalidName = name !== "" && !isValidAgentName(name)
  const submitDisabled =
    form.submitting ||
    invalidName ||
    form.url.trim() === "" ||
    form.path.trim() === ""

  function handleSubmit() {
    if (!submitDisabled) submitCloneProject()
  }

  function handleEnter(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") return
    event.preventDefault()
    handleSubmit()
  }

  return (
    <Dialog open onOpenChange={(open) => !open && closeCloneProject()}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Clone a repository</DialogTitle>
          <DialogDescription>
            Paste the address of a git remote. dux clones its default branch,
            adds it as a project and starts an agent there. It never asks for a
            password: the credentials and ssh keys you already use in a shell
            are the ones it uses.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="clone-address" className="text-sm font-medium">
            Repository address
          </label>
          <Input
            id="clone-address"
            value={form.url}
            onChange={(event) => setCloneProjectUrl(event.target.value)}
            onKeyDown={handleEnter}
            placeholder="https://github.com/owner/repo.git"
            autoFocus
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="clone-destination" className="text-sm font-medium">
            Destination folder
          </label>
          <Input
            id="clone-destination"
            value={form.path}
            onChange={(event) => setCloneProjectPath(event.target.value)}
            onKeyDown={handleEnter}
            placeholder="/path/to/the/new/folder"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <p className="text-sm font-medium">First agent</p>
          <AgentNameInput
            value={form.name}
            onChange={setCloneProjectName}
            onSubmit={handleSubmit}
            placeholder="Agent name (optional)"
            ariaLabel="Agent name"
            invalid={invalidName}
            generating={form.namePending}
            autoFocus={false}
          />
          <p className="text-xs text-muted-foreground">
            Letters, digits, dashes, underscores and slashes. This becomes the
            branch name.
          </p>
        </div>
        <RandomizeNameCheckbox
          checked={form.randomize}
          onToggle={toggleCloneProjectRandomize}
        />
        {form.error && (
          <p role="alert" className="text-destructive text-sm">
            {form.error}
          </p>
        )}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={closeCloneProject}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={submitDisabled}>
            Clone
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
