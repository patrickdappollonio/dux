import { Button } from "@/components/ui/button"
import {
  AgentNameInput,
  RandomizeNameCheckbox,
} from "@/components/AgentNameFields"
import { Modal, useHeldWhileClosed } from "@/components/modal"
import { CheckboxRow } from "@/components/CheckboxRow"
import { ReviewCopyQuestion } from "@/components/ReviewCopyQuestion"
import { FormGroup, type FieldWiring } from "@/components/ui/form-group"
import { Input } from "@/components/ui/input"
import { renderProse } from "@/lib/prose"
import type { KeyboardEvent } from "react"
import {
  createAgentDialogView,
  createAgentFormView,
} from "@/components/createAgentDialogView"
import type { CreateAgentDialogView } from "@/components/createAgentDialogView"
import {
  closeCreateAgent,
  closeExistingBranch,
  openNewAgentPicker,
  setCreateAgentDraft,
  setCreateAgentPrInput,
  setCreateAgentReviewCopyAgreed,
  setPendingPrReference,
  submitNameDialog,
  toggleCreateAgentCopyChanges,
  toggleCreateAgentRandomize,
  useDux,
} from "@/lib/store"

export function CreateAgentDialog() {
  const live = useDux()
  const open = live.createAgentTarget !== null
  // What the dialog showed when the close began, kept while the close animates.
  const createAgentTarget = useHeldWhileClosed(open, live.createAgentTarget)
  const createAgentDraft = useHeldWhileClosed(open, live.createAgentDraft)
  const createAgentRandomize = useHeldWhileClosed(open, live.createAgentRandomize)
  const createAgentCopyChanges = useHeldWhileClosed(open, live.createAgentCopyChanges)
  const createAgentNamePending = useHeldWhileClosed(open, live.createAgentNamePending)
  const createAgentPrInput = useHeldWhileClosed(open, live.createAgentPrInput)
  const createAgentPrResolving = useHeldWhileClosed(open, live.createAgentPrResolving)
  const createAgentSubmitting = useHeldWhileClosed(open, live.createAgentSubmitting)
  const createAgentPrError = useHeldWhileClosed(open, live.createAgentPrError)
  const createAgentReviewCopy = useHeldWhileClosed(open, live.createAgentReviewCopy)
  const spine = useHeldWhileClosed(open, live.spine)
  const dialog = createAgentDialogView(createAgentTarget, spine)
  const form = createAgentFormView(
    dialog.kind,
    createAgentDraft,
    createAgentPrInput,
    createAgentPrResolving,
    createAgentReviewCopy !== null && !createAgentReviewCopy.agreed,
  )

  function handleSubmit() {
    if (form.submitDisabled) return
    submitNameDialog(createAgentDraft.trim())
  }

  return (
    <Modal
      kind="form"
      open={open && dialog.open}
      onClose={closeCreateAgent}
      // While the server's question about an existing branch is up over the
      // form, every way out of the form, Back included, answers that question
      // instead, and the form stays with what was typed.
      beforeClose={() => {
        if (live.existingBranchTarget === null) return "close"
        closeExistingBranch()
        return "stay"
      }}
      // Reopened by the server's answer rather than by a click, in which case it
      // takes no history entry for the browser's Back.
      historyEntry={createAgentReviewCopy === null}
      title={renderProse(dialog.title)}
      description={dialog.description}
      submit={{
        label: createAgentPrResolving ? "Finding the project…" : dialog.submitLabel,
        tone: "primary",
        onSelect: handleSubmit,
        busy: createAgentPrResolving || createAgentSubmitting,
        disabledReason: form.submitRefusal,
      }}
    >
      <PrReferenceFields
        dialog={dialog}
        value={createAgentPrInput}
        error={createAgentPrError}
        onSubmit={handleSubmit}
      />
      <BranchNameField
        dialog={dialog}
        value={createAgentDraft}
        invalid={form.invalidName}
        generating={createAgentNamePending}
        onSubmit={handleSubmit}
      />
      <AgentOptions
        showCopyChanges={dialog.showCopyChanges}
        randomize={createAgentRandomize}
        copyChanges={createAgentCopyChanges}
      />
      {createAgentReviewCopy && (
        <ReviewCopyQuestion
          warning={createAgentReviewCopy.warning}
          agreed={createAgentReviewCopy.agreed}
          onAgreedChange={setCreateAgentReviewCopyAgreed}
        />
      )}
    </Modal>
  )
}

function handleEnter(
  event: KeyboardEvent<HTMLInputElement>,
  onSubmit: () => void,
): void {
  if (event.key !== "Enter") return
  event.preventDefault()
  onSubmit()
}

function chooseExistingProject(reference: string): void {
  setPendingPrReference(reference.trim() || null)
  closeCreateAgent()
  openNewAgentPicker("from_pr")
}

const BRANCH_RULES = "Any name git accepts for a branch. This becomes the branch name."

// The branch name, alone in the dialog unless a pull request's box is above it:
// alone it is bare, named for a screen reader; beside that box both are labelled.
function BranchNameField({
  dialog,
  value,
  invalid,
  generating,
  onSubmit,
}: {
  dialog: CreateAgentDialogView
  value: string
  invalid: boolean
  generating: boolean
  onSubmit: () => void
}) {
  const input = (field: FieldWiring, placeholder?: string) => (
    <AgentNameInput
      field={field}
      value={value}
      onChange={setCreateAgentDraft}
      onSubmit={onSubmit}
      placeholder={placeholder}
      generating={generating}
      autoFocus={false}
    />
  )
  return dialog.showPrFields ? (
    <FormGroup
      layout="stacked"
      control="single"
      label="Branch name"
      optional={dialog.nameOptional || undefined}
      notes={[{ words: BRANCH_RULES }]}
      invalid={invalid}
    >
      {(field) => input(field)}
    </FormGroup>
  ) : (
    <FormGroup layout="stacked" control="single" spokenName="Branch name" help={BRANCH_RULES} invalid={invalid}>
      {(field) => input(field, dialog.nameOptional ? "Branch name (optional)" : "Branch name")}
    </FormGroup>
  )
}

interface PrReferenceFieldsProps {
  dialog: CreateAgentDialogView
  value: string
  error: string | null
  onSubmit: () => void
}

function PrReferenceFields({
  dialog,
  value,
  error,
  onSubmit,
}: PrReferenceFieldsProps) {
  if (!dialog.showPrFields) return null
  return (
    <>
      <FormGroup layout="stacked" control="single" label="GitHub pull request" error={error} errorAnnounce="alert">
        {(field) => (
          <Input
            {...field}
            value={value}
            onChange={(event) => setCreateAgentPrInput(event.target.value)}
            onKeyDown={(event) => handleEnter(event, onSubmit)}
            placeholder={dialog.prPlaceholder}
          />
        )}
      </FormGroup>
      {dialog.showProjectPicker && (
        <div className="flex justify-start">
          <Button
            variant="link"
            className="h-auto px-0 max-md:min-h-10 pointer-coarse:min-h-10 text-muted-foreground"
            onClick={() => chooseExistingProject(value)}
          >
            or choose an existing project
          </Button>
        </div>
      )}
    </>
  )
}

interface AgentOptionsProps {
  showCopyChanges: boolean
  randomize: boolean
  copyChanges: boolean
}

function AgentOptions({
  showCopyChanges,
  randomize,
  copyChanges,
}: AgentOptionsProps) {
  return (
    <>
      <RandomizeNameCheckbox
        checked={randomize}
        onToggle={toggleCreateAgentRandomize}
      />
      {showCopyChanges && (
        <CheckboxRow
          id="copy-uncommitted-changes"
          checked={copyChanges}
          onCheckedChange={toggleCreateAgentCopyChanges}
        >
          Copy uncommitted changes from the project checkout
        </CheckboxRow>
      )}
    </>
  )
}
