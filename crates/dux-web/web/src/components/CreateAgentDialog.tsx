import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { GlyphSpinner } from "@/components/GlyphSpinner"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { sanitizeAgentName } from "@/lib/agentName"
import type { ChangeEvent, KeyboardEvent } from "react"
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
          <DialogTitle>{dialog.title}</DialogTitle>
          <DialogDescription>{dialog.description}</DialogDescription>
        </DialogHeader>
        <PrReferenceFields
          dialog={dialog}
          value={createAgentPrInput}
          error={createAgentPrError}
          onSubmit={handleSubmit}
        />
        <AgentNameField
          dialog={dialog}
          value={createAgentDraft}
          invalid={form.invalidName}
          generating={createAgentNamePending}
          onSubmit={handleSubmit}
        />
        <p className="text-xs text-muted-foreground">
          Letters, digits, dashes, underscores and slashes — becomes the branch
          name.
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

function handleNameChange(event: ChangeEvent<HTMLInputElement>): void {
  const input = event.target
  const raw = input.value
  const caret = input.selectionStart ?? raw.length
  setCreateAgentDraft(raw)
  const sanitized = sanitizeAgentName(raw)
  if (sanitized === raw) return
  const next = Math.max(0, caret - (raw.length - sanitized.length))
  // Controlled sanitization moves the caret to the end; restore its adjusted
  // position after removed characters shorten a mid-string edit.
  requestAnimationFrame(() => input.setSelectionRange(next, next))
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

interface AgentNameFieldProps {
  dialog: CreateAgentDialogView
  value: string
  invalid: boolean
  generating: boolean
  onSubmit: () => void
}

function AgentNameField({
  dialog,
  value,
  invalid,
  generating,
  onSubmit,
}: AgentNameFieldProps) {
  return (
    <div className="relative">
      <Input
        value={value}
        onChange={handleNameChange}
        onKeyDown={(event) => handleEnter(event, onSubmit)}
        placeholder={dialog.namePlaceholder}
        aria-invalid={invalid}
        disabled={generating}
        autoFocus={dialog.nameAutoFocus}
      />
      {generating && (
        <GlyphSpinner className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
      )}
    </div>
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
      <div className="flex items-center gap-2">
        <Checkbox
          id="randomize-agent-name"
          checked={randomize}
          onCheckedChange={toggleCreateAgentRandomize}
        />
        <label htmlFor="randomize-agent-name" className="text-sm">
          Use randomized pet name
        </label>
      </div>
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
