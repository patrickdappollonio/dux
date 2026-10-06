import { Button } from "@/components/ui/button"
import {
  AgentNameInput,
  RandomizeNameCheckbox,
} from "@/components/AgentNameFields"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Checkbox } from "@/components/ui/checkbox"
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
  openNewAgentPicker,
  setCreateAgentDraft,
  setCreateAgentPrInput,
  setPendingPrReference,
  submitNameDialog,
  toggleCreateAgentCopyChanges,
  toggleCreateAgentRandomize,
  useDux,
} from "@/lib/store"

export function CreateAgentDialog() {
  const {
    createAgentTarget,
    createAgentDraft,
    createAgentRandomize,
    createAgentCopyChanges,
    createAgentNamePending,
    createAgentPrInput,
    createAgentPrResolving,
    createAgentPrError,
    spine,
  } = useDux()
  const dialog = createAgentDialogView(createAgentTarget, spine)
  const form = createAgentFormView(
    dialog.kind,
    createAgentDraft,
    createAgentPrInput,
    createAgentPrResolving,
  )

  function handleSubmit() {
    if (form.submitDisabled) return
    submitNameDialog(createAgentDraft.trim())
  }

  return (
    <Dialog open={dialog.open} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>{renderProse(dialog.title)}</DialogTitle>
          <DialogDescription>{dialog.description}</DialogDescription>
        </DialogHeader>
        <PrReferenceFields
          dialog={dialog}
          value={createAgentPrInput}
          error={createAgentPrError}
          onSubmit={handleSubmit}
        />
        <AgentNameInput
          value={createAgentDraft}
          onChange={setCreateAgentDraft}
          onSubmit={handleSubmit}
          placeholder={dialog.namePlaceholder}
          invalid={form.invalidName}
          generating={createAgentNamePending}
          autoFocus={dialog.nameAutoFocus}
        />
        <p className="text-xs text-muted-foreground">
          Letters, digits, dashes, underscores and slashes. This becomes the
          branch name.
        </p>
        <AgentOptions
          showCopyChanges={dialog.showCopyChanges}
          randomize={createAgentRandomize}
          copyChanges={createAgentCopyChanges}
        />
        <div className="h-2" />
        <AgentDialogFooter
          submitLabel={
            createAgentPrResolving ? "Finding the project…" : dialog.submitLabel
          }
          disabled={form.submitDisabled}
          onSubmit={handleSubmit}
        />
      </DialogContent>
    </Dialog>
  )
}

function handleOpenChange(open: boolean): void {
  if (!open) closeCreateAgent()
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
      <Input
        value={value}
        onChange={(event) => setCreateAgentPrInput(event.target.value)}
        onKeyDown={(event) => handleEnter(event, onSubmit)}
        placeholder={dialog.prPlaceholder}
        aria-label="GitHub pull request"
        aria-invalid={error !== null}
        aria-describedby={error ? "create-agent-pr-error" : undefined}
        autoFocus
      />
      {error && (
        <p
          id="create-agent-pr-error"
          role="alert"
          className="text-destructive text-sm"
        >
          {error}
        </p>
      )}
      {dialog.showProjectPicker && (
        <div className="flex justify-start">
          <Button
            variant="link"
            className="h-auto px-0 max-md:min-h-10 text-muted-foreground"
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
        <div className="flex items-center gap-2">
          <Checkbox
            id="copy-uncommitted-changes"
            checked={copyChanges}
            onCheckedChange={toggleCreateAgentCopyChanges}
          />
          <label htmlFor="copy-uncommitted-changes" className="text-sm">
            Copy uncommitted changes from the project checkout
          </label>
        </div>
      )}
    </>
  )
}

interface AgentDialogFooterProps {
  submitLabel: string
  disabled: boolean
  onSubmit: () => void
}

function AgentDialogFooter({
  submitLabel,
  disabled,
  onSubmit,
}: AgentDialogFooterProps) {
  return (
    <DialogFooter>
      <Button variant="outline" onClick={closeCreateAgent}>
        Cancel
      </Button>
      <Button onClick={onSubmit} disabled={disabled}>
        {submitLabel}
      </Button>
    </DialogFooter>
  )
}
