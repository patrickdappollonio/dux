import { useState } from "react"
import { FolderGit2 } from "lucide-react"

import { BrailleSpinner } from "@/components/BrailleSpinner"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Badge } from "@/components/ui/badge"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import { isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import {
  attachWorktree,
  closeAttachWorktree,
  useDux,
} from "@/lib/store"
import type { ProjectWorktreeEntryView } from "@/lib/types"

// The last path segment (the worktree directory's name) — what the TUI shows
// as the entry label. Falls back to the full path for a root-level path.
function pathTail(path: string): string {
  const parts = path.split("/").filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1] : path
}

// Mounted only while the dialog is open so its local select/name state resets on
// each open — no set-state-in-effect needed (matching AddProjectDialog).
function AttachWorktreeBody({ projectId }: { projectId: string }) {
  const { attachWorktreeEntries, attachWorktreeLoading, viewModel } = useDux()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")

  const project = viewModel?.projects.find((p) => p.id === projectId)
  const projectName = project?.name ?? "project"
  const adoptable = attachWorktreeEntries.filter((e) => e.adoptable)
  const attached = attachWorktreeEntries.filter((e) => !e.adoptable)

  // The display name mirrors the TUI prompt: empty is rejected (a worktree
  // adoption requires a name) and a non-empty name must pass the agent-name
  // rules. The branch already exists, so this is a display name only.
  const empty = name.trim() === ""
  const invalidNonEmpty = !empty && !isValidAgentName(name)
  const disabled = !selected || empty || invalidNonEmpty

  function handleSelect(entry: ProjectWorktreeEntryView) {
    setSelected(entry.worktree_path)
    // Default the display name to the worktree's tail, like the TUI seeds the
    // managed worktree's relative name; the user can edit it.
    if (name.trim() === "") setName(sanitizeAgentName(pathTail(entry.worktree_path)))
  }

  function handleAttach() {
    if (disabled || !selected) return
    attachWorktree(projectId, selected, name.trim())
    closeAttachWorktree()
  }

  return (
    <DialogContent className="sm:max-w-xl" showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Attach worktree in {projectName}</DialogTitle>
        <DialogDescription>
          Adopts an orphaned managed worktree (one dux created with no live
          agent) as a new agent, launching a fresh session on its existing
          branch.
        </DialogDescription>
      </DialogHeader>

      <ScrollArea className="h-[40vh] rounded-md border md:h-64">
        {attachWorktreeLoading ? (
          <div className="flex h-[40vh] items-center justify-center md:h-64">
            <BrailleSpinner className="text-lg text-muted-foreground" />
          </div>
        ) : adoptable.length === 0 && attached.length === 0 ? (
          <div className="flex h-[40vh] items-center justify-center px-6 text-center text-sm text-muted-foreground md:h-64">
            No orphaned worktrees — every worktree already has an agent.
          </div>
        ) : (
          <div className="flex flex-col">
            {adoptable.map((entry) => {
              const isSelected = selected === entry.worktree_path
              return (
                <button
                  key={entry.worktree_path}
                  type="button"
                  onClick={() => handleSelect(entry)}
                  // min-h-11 gives a ≥44px touch target on phones; desktop keeps
                  // the compact density via md:.
                  className={`flex min-h-11 items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent md:min-h-0 ${
                    isSelected ? "bg-accent" : ""
                  }`}
                >
                  <FolderGit2 className="size-4 shrink-0 text-muted-foreground" />
                  <span className="flex-1 truncate">
                    {pathTail(entry.worktree_path)}
                  </span>
                  <Badge variant="secondary" className="shrink-0 font-mono">
                    {entry.branch_name}
                  </Badge>
                </button>
              )
            })}
            {attached.length > 0 ? (
              <>
                <div className="px-3 pt-3 pb-1 text-sm font-medium text-muted-foreground">
                  Already has an agent
                </div>
                {attached.map((entry) => (
                  <SimpleTooltip
                    key={entry.worktree_path}
                    content={entry.reason ?? undefined}
                  >
                    <div className="flex min-h-11 cursor-not-allowed items-center gap-2 px-3 py-2 text-left text-sm opacity-50 md:min-h-0">
                      <FolderGit2 className="size-4 shrink-0 text-muted-foreground" />
                      <span className="flex-1 truncate">
                        {pathTail(entry.worktree_path)}
                      </span>
                      <Badge variant="outline" className="shrink-0 font-mono">
                        {entry.branch_name}
                      </Badge>
                    </div>
                  </SimpleTooltip>
                ))}
              </>
            ) : null}
          </div>
        )}
      </ScrollArea>

      {selected ? (
        <div className="grid gap-1">
          <Input
            value={name}
            onChange={(e) => {
              const el = e.target
              const raw = el.value
              const caret = el.selectionStart ?? raw.length
              setName(sanitizeAgentName(raw))
              // Restore the caret after live sanitization shrinks the string,
              // so mid-string edits don't jump to the end (same as CreateAgent).
              const sanitized = sanitizeAgentName(raw)
              if (sanitized !== raw) {
                const next = Math.max(0, caret - (raw.length - sanitized.length))
                requestAnimationFrame(() => el.setSelectionRange(next, next))
              }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                handleAttach()
              }
            }}
            placeholder="Display name"
            aria-invalid={invalidNonEmpty}
            autoFocus
          />
          <span className="text-xs text-muted-foreground">
            Display name only — the branch already exists. Letters, digits,
            dashes, underscores and slashes.
          </span>
        </div>
      ) : null}

      <DialogFooter>
        <Button variant="outline" onClick={closeAttachWorktree}>
          Cancel
        </Button>
        <Button disabled={disabled} onClick={handleAttach}>
          Attach worktree
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function AttachWorktreeDialog() {
  const { attachWorktreeTarget } = useDux()

  return (
    <Dialog
      open={attachWorktreeTarget !== null}
      onOpenChange={(o) => {
        if (!o) closeAttachWorktree()
      }}
    >
      {attachWorktreeTarget !== null && (
        <AttachWorktreeBody projectId={attachWorktreeTarget} />
      )}
    </Dialog>
  )
}
