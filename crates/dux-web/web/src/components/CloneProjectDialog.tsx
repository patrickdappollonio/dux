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
import { ReviewCopyQuestion } from "@/components/ReviewCopyQuestion"
import { isValidAgentName } from "@/lib/agentName"
import { type CloneProjectPr, reviewCopyBlocksSubmit, reviewCopyShown } from "@/lib/cloneProjectPr"
import {
  addExistingOfferProse,
  chip,
  cloneOfferIncompleteProse,
  cloneOfferProse,
  prose,
  renderProse,
} from "@/lib/prose"
import {
  chooseExistingProjectForClone,
  closeCloneProject,
  setCloneProjectName,
  setCloneProjectPath,
  setCloneProjectReviewCopyAgreed,
  setCloneProjectUrl,
  submitCloneProject,
  toggleCloneProjectRandomize,
  useDux,
} from "@/lib/store"

// The sentence a pull request's offer opens with: the clone, the clone with
// some projects unchecked, or the add of a folder that already is a checkout.
function offerBody(pr: CloneProjectPr, folder: string) {
  const number = pr.offer.number
  if (pr.folder === "checkout") return addExistingOfferProse(folder, pr.repository, number)
  return pr.incompleteSummary
    ? cloneOfferIncompleteProse(pr.repository, number, pr.incompleteSummary)
    : cloneOfferProse(pr.repository, number)
}

// Clone a repository as a new project and start its first agent, or, in
// pull-request mode, offer the pull request's repository: clone it (or add the
// folder that already holds it) and start the agent on the pull request. The
// server answers a refused check with its sentence, which stays on screen with
// the dialog open; a started clone closes it, and the progress and the one
// final ride the status toasts from there.
export function CloneProjectDialog() {
  const { cloneProject: form } = useDux()
  if (!form) return null
  const pr = form.pr
  const name = form.name.trim()
  const invalidName = name !== "" && !isValidAgentName(name)
  const submitDisabled =
    form.submitting ||
    invalidName ||
    (pr !== null && name === "") ||
    (pr === null && form.url.trim() === "") ||
    form.path.trim() === "" ||
    (pr !== null && reviewCopyBlocksSubmit(pr, name))

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
          <DialogTitle>{pr ? "New agent from PR" : "Clone a repository"}</DialogTitle>
          <DialogDescription>
            {pr
              ? renderProse(offerBody(pr, form.path.trim()))
              : "Paste the address of a git remote. dux clones its default branch, adds it as a project and starts an agent there. It never asks for a password: the credentials and ssh keys you already use in a shell are the ones it uses."}
          </DialogDescription>
          {pr && (
            <p className="text-sm text-muted-foreground">
              {renderProse(prose`${chip(pr.offer.title)} from branch ${chip(pr.offer.head_branch)}`)}
            </p>
          )}
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
            autoFocus={!pr}
            readOnly={pr !== null}
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
            autoFocus={pr !== null}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <p className="text-sm font-medium">{pr ? "Agent" : "First agent"}</p>
          <AgentNameInput
            value={form.name}
            onChange={setCloneProjectName}
            onSubmit={handleSubmit}
            placeholder={pr ? "Agent name" : "Agent name (optional)"}
            ariaLabel="Agent name"
            invalid={invalidName}
            generating={form.namePending}
            autoFocus={false}
          />
          <p className="text-xs text-muted-foreground">
            Any name git accepts for a branch. This becomes the branch
            name.
          </p>
        </div>
        {pr ? (
          <PullRequestReviewCopy pr={pr} name={name} />
        ) : (
          <RandomizeNameCheckbox
            checked={form.randomize}
            onToggle={toggleCloneProjectRandomize}
          />
        )}
        {form.error && (
          <p role="alert" className="text-destructive text-sm">
            {form.error}
          </p>
        )}
        {pr?.hasProjects && (
          <div className="flex justify-start">
            <Button
              variant="link"
              className="h-auto px-0 max-md:min-h-10 text-muted-foreground"
              onClick={chooseExistingProjectForClone}
            >
              Choose an existing project
            </Button>
          </div>
        )}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={closeCloneProject}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={submitDisabled}>
            {!pr ? "Clone" : pr.folder === "checkout" ? "Add this folder as a project" : "Clone and create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// What the dialog says about a review copy, for the name as it stands: the
// warning and the box while a copy will be needed, the refusal while no name
// is free, and nothing otherwise.
function PullRequestReviewCopy({ pr, name }: { pr: CloneProjectPr; name: string }) {
  const shown = reviewCopyShown(pr, name)
  if (shown.state === "needed") {
    return (
      <ReviewCopyQuestion
        warning={shown.warning}
        agreed={pr.reviewCopyAgreed}
        onAgreedChange={setCloneProjectReviewCopyAgreed}
      />
    )
  }
  if (shown.state === "no_free_name") return <ReviewCopyQuestion warning={shown.message} />
  return null
}
