import { useEffect, useState } from "react"
import { AlertTriangle } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import { folderName, matchWorkspace } from "@/lib/agentWorkspace"
import { renderProse } from "@/lib/prose"
import {
  renameBlockedReason,
  renameBranchNoteProse,
} from "@/lib/renameAgent"
import { sessionsApi } from "@/lib/sessionsApi"
import {
  closeRename,
  setRenameBranch,
  setRenameDraft,
  submitRename,
  useDux,
} from "@/lib/store"

// Rename an agent: the same dialog the terminal UI shows. The field pre-fills the
// current custom name (empty when none), with the branch name (a standalone
// agent's folder name) as the placeholder, because an empty name goes back to
// it. An agent with a branch also gets an unticked "Also rename the git branch"
// box, with a line saying what stays on the old name when the branch has an
// open pull request or a pushed copy.
export function RenameSessionDialog() {
  const { renameTarget, renameDraft, renameBranch, renamePending, spine } = useDux()
  const session = spine?.sessions.find((s) => s.id === renameTarget)
  const branch = session
    ? matchWorkspace<string | null>(session.workspace, {
        managed: (w) => w.branch_name,
        folder: () => null,
      })
    : null
  const fallbackName = session
    ? matchWorkspace(session.workspace, {
        managed: (w) => w.branch_name,
        folder: (w) => folderName(w.folder_path) ?? w.folder_label,
      })
    : ""
  const branchNamed = branch !== null
  const openPr = session?.pr?.state === "open" ? session.pr.number : null

  // Whether the branch has a pushed copy, tagged with the agent it was asked
  // about so an answer that lands after the dialog moved on is ignored.
  const [pushedAnswer, setPushedAnswer] = useState<{
    id: string
    pushed: boolean
  } | null>(null)
  const pushed = pushedAnswer?.id === renameTarget && pushedAnswer.pushed

  const open = useVanishedTargetGuard(
    renameTarget !== null,
    session !== undefined,
    closeRename,
  )

  useEffect(() => {
    if (!open || !branchNamed || !renameTarget) return
    let live = true
    const id = renameTarget
    sessionsApi
      .branchPushed(id)
      .then((answer) => {
        if (live) setPushedAnswer({ id, pushed: answer.pushed === true })
      })
      // No answer is no warning: the box still says what it does.
      .catch(() => {})
    return () => {
      live = false
    }
  }, [open, branchNamed, renameTarget])

  const invalidNonEmpty =
    branchNamed &&
    renameDraft.trim() !== "" &&
    !isValidAgentName(renameDraft.trim())
  const blocked = renameBlockedReason(renameDraft, renameBranch, branchNamed)
  const canSave = !invalidNonEmpty && blocked === null && !renamePending
  const warns = openPr !== null || pushed

  function handleSubmit() {
    if (!canSave) return
    void submitRename()
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
            {branchNamed
              ? "Sets the name this agent shows. Leave it empty to show its branch name again."
              : "Sets the name this agent shows. Leave it empty to show its folder's name again."}
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
            if (!branchNamed) return
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
          placeholder={fallbackName}
          aria-label="Agent name"
          aria-invalid={invalidNonEmpty}
          autoFocus
        />
        {branchNamed && (
          <p className="text-xs text-muted-foreground">
            Letters, digits, dashes, underscores and slashes.
          </p>
        )}
        {branch !== null && (
          <div className="grid gap-1">
            <div className="flex items-center gap-2">
              <Checkbox
                id="rename-branch"
                checked={renameBranch}
                onCheckedChange={(checked) => setRenameBranch(checked)}
              />
              <label htmlFor="rename-branch" className="text-sm">
                Also rename the git branch
              </label>
            </div>
            <p
              className={
                warns
                  ? "flex items-start gap-1.5 text-xs text-dux-warning"
                  : "text-xs text-muted-foreground"
              }
              data-testid="rename-branch-note"
            >
              {warns && <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />}
              <span>{renderProse(renameBranchNoteProse(branch, openPr, pushed))}</span>
            </p>
          </div>
        )}
        {blocked !== null && (
          <p className="text-xs text-destructive" role="alert">
            {blocked}
          </p>
        )}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={closeRename}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={!canSave}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
