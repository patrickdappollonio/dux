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
import { isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import {
  closeRename,
  setRenameDraft,
  submitRename,
  useDux,
} from "@/lib/store"

// Rename a session's display title. The input pre-fills the current custom title
// (empty when none), with the branch name as the placeholder so clearing the
// field is obviously the "revert to branch name" action. A non-empty title is
// validated as an agent name (same rules the server enforces); an empty title
// clears the title back to the branch name. State lives in the store, mirroring
// the new-agent dialog, so the input stays fully controlled.
export function RenameSessionDialog() {
  const { renameTarget, renameDraft, viewModel } = useDux()
  const open = renameTarget !== null
  const session = viewModel?.sessions.find((s) => s.id === renameTarget)
  const branchName = session?.branch_name ?? ""

  // Empty is allowed (clears the title). A non-empty invalid name disables save.
  const invalidNonEmpty =
    renameDraft.trim() !== "" && !isValidAgentName(renameDraft.trim())

  function handleSubmit() {
    if (invalidNonEmpty) return
    submitRename()
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeRename()
      }}
    >
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Rename agent</DialogTitle>
          <DialogDescription>
            Sets a custom display name for this agent — the git branch keeps
            its name. Clear the field to revert to showing the branch name.
          </DialogDescription>
        </DialogHeader>
        <Input
          value={renameDraft}
          onChange={(e) => {
            const el = e.target
            const raw = el.value
            const caret = el.selectionStart ?? raw.length
            setRenameDraft(raw)
            // Keep the caret put when sanitization shrinks the string (same
            // approach as the new-agent input).
            const sanitized = sanitizeAgentName(raw)
            if (sanitized !== raw) {
              const next = Math.max(0, caret - (raw.length - sanitized.length))
              requestAnimationFrame(() => el.setSelectionRange(next, next))
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault()
              handleSubmit()
            }
          }}
          placeholder={branchName || "Branch name"}
          aria-invalid={invalidNonEmpty}
          autoFocus
        />
        <p className="text-xs text-muted-foreground">
          Letters, digits, dashes, underscores and slashes. Leave empty to use
          the branch name.
        </p>
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={closeRename}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={invalidNonEmpty}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
