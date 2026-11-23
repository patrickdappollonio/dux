import { useRef, type KeyboardEvent } from "react"

import {
  AgentNameInput,
  RandomizeNameCheckbox,
} from "@/components/AgentNameFields"
import { ErrorLine } from "@/components/ErrorLine"
import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ReviewCopyQuestion } from "@/components/ReviewCopyQuestion"
import { INVALID_AGENT_NAME, isValidAgentName } from "@/lib/agentName"
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
  const open = useDux().cloneProject !== null
  const life = useModalLifetime(open)
  return life.mounted ? <CloneProjectForm key={life.epoch} open={open} onClosed={life.onClosed} /> : null
}

function CloneProjectForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  // The form as it was when the close began, kept while the close animates.
  const form = useHeldWhileClosed(open, useDux().cloneProject)
  const destinationRef = useRef<HTMLInputElement | null>(null)
  if (!form) return null
  const pr = form.pr
  const name = form.name.trim()
  const invalidName = name !== "" && !isValidAgentName(name)
  const refusal = cloneRefusal(form.url, form.path, name, invalidName, pr)
  const submitDisabled = form.submitting || refusal !== null

  function handleSubmit() {
    if (!submitDisabled) submitCloneProject()
  }

  function handleEnter(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") return
    event.preventDefault()
    handleSubmit()
  }

  return (
    <Modal
      kind="form"
      open={open}
      onClose={closeCloneProject}
      onClosed={onClosed}
      title={pr ? "New agent from PR" : "Clone a repository"}
      description={
        <>
          {pr
            ? renderProse(offerBody(pr, form.path.trim()))
            : "Paste the address of a git remote, a GitHub page from your browser, or just owner/repo. dux clones its default branch, adds it as a project and starts an agent there. It never asks for a password: the credentials and ssh keys you already use in a shell are the ones it uses."}
        </>
      }
      // The address is the pull request's and read-only in a pull request's
      // offer, so the folder is the first field there is to type in.
      initialFocus={pr ? destinationRef : undefined}
      submit={{
        label: !pr
          ? "Clone"
          : pr.folder === "checkout"
            ? "Add this folder as a project"
            : "Clone and create",
        tone: "primary",
        onSelect: handleSubmit,
        busy: form.submitting,
        disabledReason: refusal,
      }}
    >
      {pr && (
        <p className="text-sm text-muted-foreground">
          {renderProse(prose`${chip(pr.offer.title)} from branch ${chip(pr.offer.head_branch)}`)}
        </p>
      )}
      <div className="flex flex-col gap-1.5">
        <label htmlFor="clone-address" className="text-sm font-medium">
          Repository address
        </label>
        <Input
          id="clone-address"
          value={form.url}
          onChange={(event) => setCloneProjectUrl(event.target.value)}
          onKeyDown={handleEnter}
          placeholder="owner/repo or a git URL"
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
          ref={destinationRef}
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
      <ErrorLine>{form.error}</ErrorLine>
      {pr?.hasProjects && (
        <div className="flex justify-start">
          <Button
            variant="link"
            className="h-auto px-0 max-md:min-h-10 pointer-coarse:min-h-10 text-muted-foreground"
            onClick={chooseExistingProjectForClone}
          >
            Choose an existing project
          </Button>
        </div>
      )}
    </Modal>
  )
}

// Why the dialog's button is held, or null when it may be pressed.
function cloneRefusal(
  url: string,
  path: string,
  name: string,
  invalidName: boolean,
  pr: CloneProjectPr | null,
): string | null {
  if (invalidName) return INVALID_AGENT_NAME
  if (pr === null && url.trim() === "") return "Enter the repository address first."
  if (path.trim() === "") return "Enter the destination folder first."
  if (pr === null) return null
  if (name === "") return "Type a name for the agent first."
  if (!reviewCopyBlocksSubmit(pr, name)) return null
  return reviewCopyShown(pr, name).state === "no_free_name"
    ? "No review copy can be made for this name. Choose another name."
    : "Tick Create a review copy to go ahead."
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
