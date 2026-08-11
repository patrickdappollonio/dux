import { Ellipsis, Folder, FolderPlus, Search } from "lucide-react"
import { useMemo, useState } from "react"

import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  closeNewAgentPicker,
  dismissNewAgentPicker,
  openAddProject,
  openAttachWorktree,
  openCreateAgent,
  openCreateAgentFromPr,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"
import type { ProjectView } from "@/lib/types"

// The New-agent picker: the home for agent creation AND every project action now
// that the flat list has no project headers. A searchable list of ALL projects
// (agent-less ones included, since this is where their first agent is created),
// each keeping its full menu (ProjectMenuItems verbatim). Clicking a project row
// in the "new" intent opens the shared create-agent name dialog for that project
// (honoring the pet-name/copy-changes config, exactly like the TUI and the
// per-project "New agent" menu item). Provider is no longer chosen at creation on
// the web: a new agent launches on the project's default_provider and is
// retargeted afterward via the agent menu's change-provider action.
export function NewAgentPickerDialog() {
  const { newAgentPickerOpen } = useDux()
  return (
    <Dialog
      open={newAgentPickerOpen}
      onOpenChange={(open) => {
        if (!open) dismissNewAgentPicker()
      }}
    >
      {/* Flex column, and deliberately NOT overflow-hidden: the popup's base
          max-h caps it to the dynamic viewport, and when the soft keyboard
          shrinks 100dvh the list below shrinks with it (min-h-0 flex child)
          instead of the popup clipping. The earlier overflow-hidden override
          left NO scrollable element when the cap bit — a finger drag anywhere
          on the dialog moved nothing and the bottom rows were unreachable on
          phones. The base overflow-y-auto stays as the last-resort scroll for
          viewports too short for even the shrunken layout. */}
      <DialogContent className="flex flex-col gap-0 p-0 sm:max-w-lg">
        {/* Mount the stateful body only while open so its useState initializers
            re-run on each open (a fresh search / selection / provider) without a
            reset effect. */}
        {newAgentPickerOpen ? <PickerBody /> : null}
      </DialogContent>
    </Dialog>
  )
}

// Per-intent copy + the action a project row fires. "new" keeps the pick-provider-
// then-Create flow; the other two are guided "pick a project" flows that hand off
// to the existing from-PR / attach-worktree dialogs.
const INTENT_COPY = {
  new: {
    title: "New agent",
    description:
      "Pick a project to create an agent. Every project action lives in each project's menu.",
  },
  from_pr: {
    title: "New agent from PR",
    description: "Pick a project to create an agent from a pull request.",
  },
  from_worktree: {
    title: "New agent from existing worktree",
    description:
      "Pick a project to see its worktrees. Adopt an unused one as an agent, or remove one you are done with.",
  },
} as const

function PickerBody() {
  const {
    spine,
    newAgentPickerIntent,
    newAgentPickerOnlyIds,
    projectWorktreeCounts,
  } = useDux()
  // Default to "new" so a missing value (older state, a test that only sets
  // newAgentPickerOpen) still renders the standard create flow.
  const intent = newAgentPickerIntent ?? "new"
  // Narrowed to a candidate set when a pull-request reference matched several
  // projects: showing every project there would bury the two that are actually
  // checkouts of that repository.
  const projects = useMemo(() => {
    const all = spine?.projects ?? []
    if (!newAgentPickerOnlyIds) return all
    const only = new Set(newAgentPickerOnlyIds)
    return all.filter((project) => only.has(project.id))
  }, [spine, newAgentPickerOnlyIds])
  const sessions = useMemo(() => spine?.sessions ?? [], [spine])

  const [query, setQuery] = useState("")

  // Agent counts per project, derived by cross-referencing sessions (the project
  // record carries no count of its own).
  const agentCounts = useMemo(() => {
    const counts = new Map<string, number>()
    for (const session of sessions) {
      counts.set(session.project_id, (counts.get(session.project_id) ?? 0) + 1)
    }
    return counts
  }, [sessions])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (q === "") return projects
    return projects.filter((project) => project.name.toLowerCase().includes(q))
  }, [projects, query])

  // What clicking a project row does, by intent. Every intent closes this picker
  // and hands off to that project's dedicated dialog: "new" opens the shared
  // create-agent name dialog (honoring the pet-name/copy-changes config), and the
  // from-PR / from-worktree intents open their own dialogs.
  function onProjectRow(project: ProjectView) {
    if (intent === "from_pr") {
      closeNewAgentPicker()
      openCreateAgentFromPr(project.id)
      return
    }
    if (intent === "from_worktree") {
      closeNewAgentPicker()
      // `true` marks the drill-down: the Worktrees dialog then offers a Back
      // control that returns to this list, instead of Cancel being the only
      // way out of a project that turned out to have nothing in it.
      openAttachWorktree(project.id, true)
      return
    }
    closeNewAgentPicker()
    openCreateAgent(project.id)
  }

  return (
    <>
      <DialogHeader className="shrink-0 p-4 pb-3">
          <DialogTitle>{INTENT_COPY[intent].title}</DialogTitle>
          <DialogDescription>{INTENT_COPY[intent].description}</DialogDescription>
          <div className="mt-2 flex items-center gap-2 rounded-md border border-input bg-input/30 px-3 max-md:min-h-10">
            <Search className="size-4 shrink-0 text-muted-foreground" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search projects"
              aria-label="Search projects"
              className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
              autoFocus
            />
          </div>
        </DialogHeader>

        {/* Height h-72 (not max-h) so the modal never grows or shrinks with the
            result count as the user types; the list scrolls internally and the
            empty state fills the same space instead of collapsing. It is a flex
            child that may SHRINK (min-h-0, never grow) so that when the soft
            keyboard shrinks the popup's dvh cap, this list gives up the space
            and the header and Add-project footer stay on screen and tappable. */}
        <ScrollArea className="h-72 min-h-0 shrink border-t">
          <div className="p-2">
            <p className="px-2 pt-1 pb-1.5 font-mono text-xs uppercase tracking-wide text-muted-foreground">
              Choose a project
            </p>
            {filtered.length === 0 ? (
              <p className="px-2 py-4 text-sm text-muted-foreground">
                No projects match “{query}”.
              </p>
            ) : (
              filtered.map((project) => {
                const count = agentCounts.get(project.id) ?? 0
                // In the worktree intent the row is a doorway into that
                // project's worktree list, so it is labelled with what is
                // BEHIND the door. An empty project reads "none" and stays
                // clickable: it is a choice, not a surprise, and disabling it
                // would give no reason and read as broken. A count that has
                // not arrived (or could not be fetched) shows no label at all
                // rather than a misleading zero.
                const worktreeCount = projectWorktreeCounts?.[project.id]
                const rowLabel =
                  intent === "from_worktree"
                    ? projectWorktreeCounts === undefined ||
                      projectWorktreeCounts === null
                      ? null
                      : (worktreeCount ?? 0) === 0
                        ? "none"
                        : `${worktreeCount} ${worktreeCount === 1 ? "worktree" : "worktrees"}`
                    : `${count} ${count === 1 ? "agent" : "agents"}`
                return (
                  <div
                    key={project.id}
                    className={cn(
                      "flex items-center gap-2 rounded-md px-2 transition-colors max-md:min-h-10",
                      "hover:bg-accent/60",
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => onProjectRow(project)}
                      className="flex min-w-0 flex-1 items-center gap-2.5 py-2 text-left"
                    >
                      <Folder className="size-4 shrink-0 text-muted-foreground" />
                      <span className="min-w-0 flex-1 truncate text-sm">
                        {project.name}
                      </span>
                      {rowLabel ? (
                        <span className="shrink-0 font-mono text-xs text-muted-foreground">
                          {rowLabel}
                        </span>
                      ) : null}
                    </button>
                    <DropdownMenu>
                      <DropdownMenuTrigger
                        render={
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-8 shrink-0 max-md:size-10"
                            aria-label="Project actions"
                          />
                        }
                      >
                        <Ellipsis />
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <ProjectMenuItems id={project.id} />
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                )
              })
            )}
          </div>
        </ScrollArea>

        <div className="shrink-0 border-t p-2">
          <button
            type="button"
            onClick={openAddProject}
            className="flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left text-sm text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground max-md:min-h-10"
          >
            <FolderPlus className="size-4 shrink-0" />
            Add a new project…
          </button>
        </div>
    </>
  )
}
