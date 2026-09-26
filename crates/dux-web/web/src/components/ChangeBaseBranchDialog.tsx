import { GitBranch, Search, TriangleAlert } from "lucide-react"
import {
  Fragment,
  useCallback,
  useMemo,
  useState,
  type ReactNode,
} from "react"

import { GlyphSpinner } from "@/components/GlyphSpinner"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { sessionLabel, workspaceDirectory } from "@/lib/agentWorkspace"
import { changeBaseBranchProse } from "@/lib/changeBaseBranch"
import { renderProse } from "@/lib/prose"
import {
  changeBaseBranch,
  closeChangeBaseBranch,
  useDux,
  type ChangeBaseBranchListing,
} from "@/lib/store"
import type { BranchChoiceView, ProjectView, SessionView } from "@/lib/types"

// "Change base branch…": pick any branch of the project, local or only on
// origin, as the one new agents start from. The server switches the project
// folder to it (creating the local branch first when only origin has it) and
// then saves it as the base, so the pick is confirmed before anything moves.
//
// The confirmation is a sibling dialog over the picker, so cancelling it lands
// back on the list, the way the Worktrees dialog's confirmation does.
export function ChangeBaseBranchDialog() {
  const { changeBaseBranchTarget, changeBaseBranchListing, spine } = useDux()
  const project = spine?.projects.find((p) => p.id === changeBaseBranchTarget)
  // The branch picked and awaiting confirmation. Every close path resets it,
  // or the next open would start on the last one's confirmation.
  const [picked, setPicked] = useState<string | null>(null)
  const close = useCallback(() => {
    setPicked(null)
    closeChangeBaseBranch()
  }, [])
  // Switching the folder of a project that no longer exists is moot.
  const open = useVanishedTargetGuard(
    changeBaseBranchTarget !== null,
    project !== undefined,
    close,
  )

  function handleConfirm() {
    if (!project || picked === null) return
    changeBaseBranch(project.id, picked)
    close()
  }

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(o) => {
          if (!o) close()
        }}
      >
        <DialogContent className="flex flex-col gap-0 p-0 sm:max-w-lg">
          {open && project ? (
            <PickerBody
              project={project}
              sessions={spine?.sessions ?? []}
              listing={changeBaseBranchListing}
              onPick={setPicked}
            />
          ) : null}
        </DialogContent>
      </Dialog>
      <Dialog
        open={open && picked !== null}
        onOpenChange={(o) => {
          if (!o) setPicked(null)
        }}
      >
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Change base branch?</DialogTitle>
            <DialogDescription>
              {project && picked !== null
                ? renderProse(
                    changeBaseBranchProse(
                      project.name,
                      project.leading_branch,
                      picked,
                    ),
                  )
                : null}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPicked(null)}>
              Cancel
            </Button>
            {/* The terminal UI's confirm button reads the same. */}
            <Button onClick={handleConfirm}>Change base branch</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

// Mounted only while the picker is open, so the search starts empty each time.
function PickerBody({
  project,
  sessions,
  listing,
  onPick,
}: {
  project: ProjectView
  sessions: SessionView[]
  listing: ChangeBaseBranchListing
  onPick: (branch: string) => void
}) {
  const [query, setQuery] = useState("")
  const filtered = useMemo(() => {
    const branches = listing.kind === "loaded" ? listing.branches : []
    const q = query.trim().toLowerCase()
    if (q === "") return branches
    return branches.filter((b) => b.name.toLowerCase().includes(q))
  }, [listing, query])

  // A held branch is named by what holds it: the agent whose worktree it is,
  // or, for a worktree no agent owns, its folder. Either is a name in a
  // sentence, so it is the shared chip.
  function holder(path: string): ReactNode {
    const session = sessions.find((s) => workspaceDirectory(s.workspace) === path)
    return session ? (
      <>
        in use by <InlineCode>{sessionLabel(session)}</InlineCode>
      </>
    ) : (
      <>
        checked out at <InlineCode>{path}</InlineCode>
      </>
    )
  }

  return (
    <>
      <DialogHeader className="shrink-0 p-4 pb-3">
        <DialogTitle>Change base branch</DialogTitle>
        <DialogDescription>
          Pick the branch new agents in <InlineCode>{project.name}</InlineCode>{" "}
          start from. dux switches the project folder to it first.{" "}
          {project.leading_branch === null
            ? "No base branch is recorded yet."
            : null}
        </DialogDescription>
        <div className="mt-2 flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 max-md:min-h-10">
          <Search className="size-4 shrink-0 text-muted-foreground" />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search branches"
            aria-label="Search branches"
            className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
            autoFocus
          />
        </div>
        {listing.kind === "loaded" && !listing.fetched && listing.fetchError ? (
          <p className="mt-1 flex items-start gap-1.5 text-xs text-amber-500">
            <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
            Not fetched from origin just now:{" "}
            {listing.fetchError.replace(/\.$/, "")}.
          </p>
        ) : null}
      </DialogHeader>

      {/* The New agent picker's fixed-height, shrinkable list, for the same
          reasons: no resizing as the user types, and room for the soft
          keyboard. */}
      <ScrollArea className="h-72 min-h-0 shrink border-t">
        <div className="p-2">
          {listing.kind === "loading" ? (
            <p className="flex items-center gap-2 px-2 py-4 text-sm text-muted-foreground">
              <GlyphSpinner />
              Fetching origin and listing the branches…
            </p>
          ) : listing.kind === "failed" ? (
            <p className="px-2 py-4 text-sm text-destructive">
              {listing.message}
            </p>
          ) : filtered.length === 0 ? (
            <p className="px-2 py-4 text-sm text-muted-foreground">
              {query.trim() === "" ? (
                "This project has no branches to switch to."
              ) : (
                <>
                  No branches match <InlineCode>{query}</InlineCode>.
                </>
              )}
            </p>
          ) : (
            filtered.map((branch) => (
              <BranchRow
                key={`${branch.location}:${branch.name}`}
                branch={branch}
                isBase={branch.name === project.leading_branch}
                heldBy={branch.held_by === null ? null : holder(branch.held_by)}
                onPick={onPick}
              />
            ))
          )}
        </div>
      </ScrollArea>
    </>
  )
}

function BranchRow({
  branch,
  isBase,
  heldBy,
  onPick,
}: {
  branch: BranchChoiceView
  isBase: boolean
  heldBy: ReactNode | null
  onPick: (branch: string) => void
}) {
  // What sits after the name, quietest last: why it cannot be picked, then
  // what is special about it.
  const notes = [
    heldBy,
    isBase ? "current base" : null,
    branch.location === "remote" ? "only on origin" : null,
  ].filter((note) => note !== null)
  return (
    <button
      type="button"
      data-testid="branch-row"
      data-branch={branch.name}
      disabled={heldBy !== null}
      onClick={() => onPick(branch.name)}
      className="flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-2 text-left transition-colors hover:bg-accent/60 disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent max-md:min-h-10"
    >
      <GitBranch className="size-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate font-mono text-sm">
        {branch.name}
      </span>
      {notes.length > 0 ? (
        <span className="min-w-0 shrink truncate text-xs text-muted-foreground">
          {notes.map((note, i) => (
            <Fragment key={i}>
              {i > 0 ? " · " : null}
              {note}
            </Fragment>
          ))}
        </span>
      ) : null}
    </button>
  )
}
