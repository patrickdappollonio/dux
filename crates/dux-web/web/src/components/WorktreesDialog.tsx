import { useState, type ReactNode } from "react"
import { Ellipsis, FileWarning, FolderGit2, Trash2 } from "lucide-react"

import { BrailleSpinner } from "@/components/BrailleSpinner"
import { SimpleTooltip } from "@/components/SimpleTooltip"
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { isValidAgentName, sanitizeAgentName } from "@/lib/agentName"
import {
  attachWorktree,
  closeAttachWorktree,
  closeDeleteWorktree,
  deleteProjectWorktree,
  openDeleteWorktree,
  openNewAgentPicker,
  useDux,
} from "@/lib/store"
import type { ProjectWorktreeEntryView } from "@/lib/types"

// The last path segment (the worktree directory's name) — what the TUI shows
// as the entry label. Falls back to the full path for a root-level path.
function pathTail(path: string): string {
  const parts = path.split("/").filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1] : path
}

// The row body truncates both the branch and the path, so the hover tooltip
// surfaces the full values (plus any disabled reason for attached rows) to keep
// the clipped text recoverable.
function rowTooltip(entry: ProjectWorktreeEntryView): ReactNode {
  return (
    <span className="flex flex-col gap-0.5">
      {entry.reason ? <span>{entry.reason}</span> : null}
      <span className="font-mono">{entry.branch_name}</span>
      <span>{entry.worktree_path}</span>
    </span>
  )
}

// The shared row body: the folder icon plus the worktree's BRANCH stacked over
// its PATH, with a marker when it holds uncommitted work. `heldBy` is the
// display name of the agent attached to it, which turns the second line into
// the pointer an attached row offers in place of a delete action.
function WorktreeRowBody({
  entry,
  heldBy,
}: {
  entry: ProjectWorktreeEntryView
  heldBy?: string
}) {
  return (
    <>
      <FolderGit2 className="size-4 shrink-0 text-muted-foreground" />
      {/* Stack the branch over the path so neither competes for the row's
         horizontal room; min-w-0 lets both truncate with an ellipsis instead of
         overflowing the fixed-width dialog and forcing a horizontal scrollbar. */}
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="truncate font-mono text-sm">{entry.branch_name}</span>
        <span className="truncate text-xs text-muted-foreground">
          {entry.worktree_path}
        </span>
        {heldBy ? (
          <span className="truncate text-xs text-muted-foreground">
            Held by {heldBy}
          </span>
        ) : null}
      </div>
      {entry.dirty ? (
        <SimpleTooltip content="This worktree has changes that are not committed anywhere.">
          {/* Amber, the warning tone the toast layer already uses, and paired
             with an icon so colour is never the only signal. */}
          <span className="flex shrink-0 items-center gap-1 text-xs text-amber-500">
            <FileWarning className="size-3.5" />
            Uncommitted changes
          </span>
        </SimpleTooltip>
      ) : null}
    </>
  )
}

// The confirmation for removing a worktree. dux removes it with
// `git worktree remove --force` and there is no trash, so this names the branch
// and the FULL path, says what is lost, and says it cannot be undone. A dirty
// worktree gets its own sentence rather than a generic warning, because "there
// is work in here that exists nowhere else" is the whole reason to stop.
//
// The branch checkbox DEFAULTS ON. Leaving a worktree's branch behind is what
// makes recreating an agent under that name fail with "branch already exists",
// and by the time someone is in this dialog they mean to be rid of the thing;
// the dialog already says the removal is forcible and unrecoverable, so the
// branch is not a bigger step than the one being confirmed. The SERVER still
// defaults to false, so a request that says nothing never deletes a branch.
function ConfirmDeleteWorktree() {
  const { deleteWorktreeTarget, attachWorktreeEntries } = useDux()
  // The component stays mounted across opens, so every close path resets this
  // to its default, or the next confirmation opens carrying the last answer.
  const [deleteBranch, setDeleteBranch] = useState(true)
  // The listing is refetched after every delete, so it is a live truth this
  // dialog can close itself on: a worktree that left the list (removed from
  // another tab, adopted by an agent) is no longer this dialog's subject.
  const stillListed =
    deleteWorktreeTarget !== null &&
    attachWorktreeEntries.some(
      (e) =>
        e.worktree_path === deleteWorktreeTarget.entry.worktree_path &&
        e.adoptable,
    )
  const open = useVanishedTargetGuard(
    deleteWorktreeTarget !== null,
    stillListed,
    () => {
      setDeleteBranch(true)
      closeDeleteWorktree()
    },
  )
  const entry = deleteWorktreeTarget?.entry
  // A detached worktree has no branch, so there is no choice to offer and the
  // request must not ask for one.
  const branch = entry?.branch ?? null

  function handleCancel() {
    setDeleteBranch(true)
    closeDeleteWorktree()
  }

  function handleConfirm() {
    if (!deleteWorktreeTarget) return
    deleteProjectWorktree(
      deleteWorktreeTarget.projectId,
      deleteWorktreeTarget.entry.worktree_path,
      branch !== null && deleteBranch,
    )
    handleCancel()
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) handleCancel()
      }}
    >
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Delete the worktree for {entry?.branch_name}?</DialogTitle>
        </DialogHeader>
        <div
          data-testid="delete-worktree-confirm"
          className="grid gap-2 text-sm text-destructive"
        >
          <p>
            <span className="font-mono">{entry?.worktree_path}</span> will be
            removed from disk. This action cannot be undone: dux has no trash and
            removes the directory forcibly.
          </p>
          {entry?.dirty ? (
            <p>
              This worktree has uncommitted changes, and they go with it. Nothing
              in there that is not committed exists anywhere else.
            </p>
          ) : null}
          {branch === null ? (
            <p className="text-muted-foreground">
              This worktree is not on a branch, so there is no branch to keep or
              delete. Only the working directory is removed.
            </p>
          ) : deleteBranch ? (
            <p>
              The branch <span className="font-mono">{branch}</span> will be
              deleted with it, forcibly. Any commits on it that are not merged
              anywhere else go too.
            </p>
          ) : (
            <p className="text-muted-foreground">
              The branch <span className="font-mono">{branch}</span> is kept.
              Only the working directory is removed.
            </p>
          )}
        </div>
        {branch !== null ? (
          <div className="flex items-center gap-2">
            <Checkbox
              id="delete-worktree-branch"
              checked={deleteBranch}
              onCheckedChange={setDeleteBranch}
            />
            <label htmlFor="delete-worktree-branch" className="text-sm">
              Also delete the branch {branch}
            </label>
          </div>
        ) : null}
        {/* Misclick-safe spacing between the warning (and the checkbox, which
           must not sit flush against the buttons) and the footer. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={handleCancel}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Delete worktree
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// Mounted only while the dialog is open so its local select/name state resets on
// each open — no set-state-in-effect needed (matching AddProjectDialog).
function WorktreesBody({ projectId }: { projectId: string }) {
  const {
    attachWorktreeEntries,
    attachWorktreeLoading,
    attachWorktreeFromPicker,
    spine,
  } = useDux()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState("")

  const project = spine?.projects.find((p) => p.id === projectId)
  const projectName = project?.name ?? "project"
  const adoptable = attachWorktreeEntries.filter((e) => e.adoptable)
  const attached = attachWorktreeEntries.filter((e) => !e.adoptable)

  // Resolve the holding agent's display name from the spine, the one place the
  // web derives an agent's name (`title || branch_name`).
  function agentName(entry: ProjectWorktreeEntryView): string | undefined {
    const session = spine?.sessions.find((s) => s.id === entry.agent_id)
    if (!session) return undefined
    return session.title || session.branch_name
  }

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

  // Back goes UP to the project list; it is offered only when the user came down
  // from there. Cancel still closes the whole flow.
  function handleBack() {
    closeAttachWorktree()
    openNewAgentPicker("from_worktree")
  }

  return (
    <DialogContent className="sm:max-w-xl" showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Worktrees in {projectName}</DialogTitle>
        <DialogDescription>
          Every worktree dux manages for this project. Pick an unused one to
          start an agent on its existing branch, or remove one you are done with.
        </DialogDescription>
      </DialogHeader>

      <ScrollArea className="h-[40vh] rounded-md border md:h-64">
        {attachWorktreeLoading ? (
          <div className="flex h-[40vh] items-center justify-center md:h-64">
            <BrailleSpinner className="text-lg text-muted-foreground" />
          </div>
        ) : adoptable.length === 0 && attached.length === 0 ? (
          <div className="flex h-[40vh] items-center justify-center px-6 text-center text-sm text-muted-foreground md:h-64">
            This project has no worktrees dux manages yet. Create an agent and
            one appears here.
          </div>
        ) : (
          <div className="flex flex-col">
            {adoptable.map((entry) => {
              const isSelected = selected === entry.worktree_path
              return (
                <div
                  key={entry.worktree_path}
                  className={`flex min-h-11 w-full items-center gap-1 pr-1 md:min-h-0 ${
                    isSelected ? "bg-accent" : "hover:bg-accent/60"
                  }`}
                >
                  <SimpleTooltip content={rowTooltip(entry)}>
                    {/* min-h-11 gives a ≥44px touch target on phones; desktop
                       keeps the compact density via md:. */}
                    <button
                      type="button"
                      onClick={() => handleSelect(entry)}
                      className="flex min-h-11 min-w-0 flex-1 items-center gap-2.5 px-3 py-2 text-left md:min-h-0"
                    >
                      <WorktreeRowBody entry={entry} />
                    </button>
                  </SimpleTooltip>
                  <DropdownMenu>
                    <DropdownMenuTrigger
                      render={
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-8 shrink-0 max-md:size-10"
                          aria-label="Worktree actions"
                        />
                      }
                    >
                      <Ellipsis />
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      {/* Neutral colour: the trailing ellipsis and the
                         confirmation carry the danger, per the row-menu
                         convention. */}
                      <DropdownMenuItem
                        onClick={() => openDeleteWorktree(projectId, entry)}
                      >
                        <Trash2 />
                        Delete worktree…
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              )
            })}
            {attached.length > 0 ? (
              <>
                <div className="px-3 pt-3 pb-1 text-sm font-medium text-muted-foreground">
                  Already has an agent
                  <span className="block text-xs font-normal">
                    To remove one of these, delete the agent holding it.
                  </span>
                </div>
                {/* Deliberately NO delete action here. Removing a worktree from
                   under a live agent is how you get a broken session, and that
                   path already exists and already confirms: delete the agent.
                   A second, worse route to the same outcome is not worth
                   having, so the row points at the agent instead. */}
                {attached.map((entry) => (
                  <SimpleTooltip
                    key={entry.worktree_path}
                    content={rowTooltip(entry)}
                  >
                    <div className="flex min-h-11 cursor-not-allowed items-center gap-2.5 px-3 py-2 text-left opacity-70 md:min-h-0">
                      <WorktreeRowBody entry={entry} heldBy={agentName(entry)} />
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
        {attachWorktreeFromPicker ? (
          <Button variant="outline" onClick={handleBack}>
            Back
          </Button>
        ) : null}
        <Button variant="outline" onClick={closeAttachWorktree}>
          Cancel
        </Button>
        <Button disabled={disabled} onClick={handleAttach}>
          Create agent
        </Button>
      </DialogFooter>
    </DialogContent>
  )
}

// The per-project worktree manager: it lists every worktree dux manages for a
// project, adopts an unused one as an agent (the "Create agent" button), and
// removes one that is no longer wanted. Its store surface keeps the older
// "attach worktree" naming from when adoption was its only job, so grep
// `attachWorktree` / `openAttachWorktree` / `attachWorktreeTarget`
// (lib/store.ts) to find the wiring behind these labels.
export function WorktreesDialog() {
  const { attachWorktreeTarget, spine } = useDux()
  const project = spine?.projects.find((p) => p.id === attachWorktreeTarget)
  // Closes the dialog when the project vanishes from the ViewModel: managing
  // the worktrees of a deleted project is moot. See the hook.
  const open = useVanishedTargetGuard(
    attachWorktreeTarget !== null,
    project !== undefined,
    closeAttachWorktree,
  )

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(o) => {
          if (!o) closeAttachWorktree()
        }}
      >
        {open && attachWorktreeTarget !== null && (
          <WorktreesBody projectId={attachWorktreeTarget} />
        )}
      </Dialog>
      {/* A sibling, not a child: the manager stays open behind the
         confirmation, so cancelling lands the user back on the list. */}
      <ConfirmDeleteWorktree />
    </>
  )
}
