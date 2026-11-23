import { GitBranch, TriangleAlert } from "lucide-react"
import { Fragment, useMemo, useState, type ReactNode } from "react"

import { LoadingSentence } from "@/components/Spinner"
import { ErrorLine } from "@/components/ErrorLine"
import { MatchHighlight } from "@/components/MatchHighlight"
import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { EmptyState } from "@/components/ui/empty"
import { useSearchPick } from "@/hooks/use-search-pick"
import { sessionLabel, workspaceDirectory } from "@/lib/agentWorkspace"
import { changeBaseBranchProse } from "@/lib/changeBaseBranch"
import { ACTIVE_ROW_RING } from "@/lib/focusRing"
import { renderProse } from "@/lib/prose"
import { matchCharRanges } from "@/lib/searchMatch"
import {
  changeBaseBranch,
  closeChangeBaseBranch,
  useDux,
} from "@/lib/store"
import type { BranchChoiceView } from "@/lib/types"
import { cn } from "@/lib/utils"

// "Change base branch…": pick any branch of the project, local or only on
// origin, as the one new agents start from. The server switches the project
// folder to it (creating the local branch first when only origin has it) and
// then saves it as the base, so the pick is confirmed before anything moves.
//
// The confirmation is a sibling dialog over the picker, so cancelling it lands
// back on the list, the way the Worktrees dialog's confirmation does.
export function ChangeBaseBranchDialog() {
  const { changeBaseBranchTarget } = useDux()
  const open = changeBaseBranchTarget !== null
  const life = useModalLifetime(open)
  // Keyed by the open, so the search and the pick start empty each time.
  return life.mounted ? (
    <BranchPicker key={life.epoch} open={open} onClosed={life.onClosed} />
  ) : null
}

function BranchPicker({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { changeBaseBranchTarget, changeBaseBranchListing, spine } = useDux()
  // The listing as it was when the close began, kept while the close animates.
  const listing = useHeldWhileClosed(open, changeBaseBranchListing)
  // The project this open is about, kept while the close animates.
  const [targetId] = useState(changeBaseBranchTarget)
  const project = spine?.projects.find((p) => p.id === targetId)
  const sessions = spine?.sessions ?? []
  // The branch picked and awaiting confirmation.
  const [picked, setPicked] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const filtered = useMemo(() => {
    const branches = listing.kind === "loaded" ? listing.branches : []
    const q = query.trim().toLowerCase()
    if (q === "") return branches
    return branches.filter((b) => b.name.toLowerCase().includes(q))
  }, [listing, query])
  // A held branch cannot be picked, so the keyboard skips it as a click would.
  const pickable = useMemo(
    () => filtered.filter((b) => b.held_by === null),
    [filtered],
  )
  const pick = useSearchPick({
    query,
    items: pickable,
    keyOf: branchKey,
    onPick: (branch) => setPicked(branch.name),
  })

  function handleConfirm() {
    if (!project || picked === null) return
    changeBaseBranch(project.id, picked)
    closeChangeBaseBranch()
  }

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
      <Modal
        kind="picker"
        open={open}
        // Switching the folder of a project that no longer exists is moot.
        vanished={open && project === undefined}
        onClose={closeChangeBaseBranch}
        onClosed={onClosed}
        size="lg"
        title="Change base branch"
        description={
          <>
            Pick the branch new agents in <InlineCode>{project?.name ?? ""}</InlineCode> start
            from. dux switches the project folder to it first.{" "}
            {project?.leading_branch === null ? "No base branch is recorded yet." : null}
          </>
        }
        search={{
          value: query,
          onChange: setQuery,
          label: "Search branches",
          fieldProps: pick.fieldProps,
        }}
        // No resizing as the user types, and room for the soft keyboard.
        steadyHeight
        toolbar={
          listing.kind === "loaded" && !listing.fetched && listing.fetchError ? (
            <p className="flex items-start gap-1.5 text-xs text-dux-warning">
              <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
              Not fetched from origin just now: {listing.fetchError.replace(/\.$/, "")}.
            </p>
          ) : undefined
        }
      >
        {listing.kind === "loading" ? (
          <p className="px-2 py-4">
            <LoadingSentence spinner="glyph" size="sm">
              Fetching origin and listing the branches…
            </LoadingSentence>
          </p>
        ) : listing.kind === "failed" ? (
          <div className="px-2 py-4">
            <ErrorLine>{listing.message}</ErrorLine>
          </div>
        ) : filtered.length === 0 ? (
          <EmptyState
            size="compact"
            title={
              query.trim() === "" ? (
                "This project has no branches to switch to."
              ) : (
                <>
                  No branches match <InlineCode>{query}</InlineCode>.
                </>
              )
            }
          />
        ) : project ? (
          filtered.map((branch) => (
            <BranchRow
              key={branchKey(branch)}
              id={pick.rowId(branchKey(branch))}
              active={pick.activeKey === branchKey(branch)}
              branch={branch}
              isBase={branch.name === project.leading_branch}
              heldBy={branch.held_by === null ? null : holder(branch.held_by)}
              onPick={setPicked}
              needle={query.trim().toLowerCase()}
            />
          ))
        ) : null}
      </Modal>
      <Modal
        kind="confirm"
        open={open && picked !== null}
        onClose={() => setPicked(null)}
        title="Change base branch?"
        description={
          project && picked !== null
            ? renderProse(changeBaseBranchProse(project.name, project.leading_branch, picked))
            : null
        }
        cancelLabel="Cancel"
        // The terminal UI's confirm button reads the same.
        confirm={{ label: "Change base branch", tone: "primary", onSelect: handleConfirm }}
      />
    </>
  )
}

function branchKey(branch: BranchChoiceView): string {
  return `${branch.location}:${branch.name}`
}

function BranchRow({
  id,
  active,
  branch,
  isBase,
  heldBy,
  onPick,
  needle,
}: {
  id: string
  // The row Enter in the search field would pick, ringed as if focused.
  active: boolean
  branch: BranchChoiceView
  isBase: boolean
  heldBy: ReactNode | null
  onPick: (branch: string) => void
  // The filter's normalized query, lit in the name it matched.
  needle: string
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
      id={id}
      data-testid="branch-row"
      data-branch={branch.name}
      disabled={heldBy !== null}
      onClick={() => onPick(branch.name)}
      className={cn(
        "flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-2 text-left transition-colors hover:bg-accent/60 disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent max-md:min-h-10 pointer-coarse:min-h-10",
        active && ACTIVE_ROW_RING,
      )}
    >
      <GitBranch className="size-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate font-mono text-sm">
        <MatchHighlight text={branch.name} ranges={matchCharRanges(branch.name, [needle])} />
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
