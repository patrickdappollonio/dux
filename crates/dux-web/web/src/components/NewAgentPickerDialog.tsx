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
  createAgentInProject,
  openAddProject,
  openAttachWorktree,
  openCreateAgentFromPr,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"
import type { ProjectView } from "@/lib/types"

// The New-agent picker: the home for agent creation AND every project action now
// that the flat list has no project headers. A searchable list of ALL projects
// (agent-less ones included, since this is where their first agent is created),
// each keeping its full ⋯ menu (ProjectMenuItems verbatim). Selecting a project +
// a provider + Create spawns the agent through the shared `createAgentInProject`
// store action; the per-project "New agent…" item in the ⋯ menu remains the path
// to the richer name dialog (custom branch name, copy-changes, pet-name toggle).
export function NewAgentPickerDialog() {
  const { newAgentPickerOpen } = useDux()
  return (
    <Dialog
      open={newAgentPickerOpen}
      onOpenChange={(open) => {
        if (!open) closeNewAgentPicker()
      }}
    >
      <DialogContent className="gap-0 overflow-hidden p-0 sm:max-w-lg">
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
      "Pick a project and a provider, then create. Every project action lives in each project's menu.",
  },
  from_pr: {
    title: "New agent from PR",
    description: "Pick a project to create an agent from a pull request.",
  },
  from_worktree: {
    title: "New agent from existing worktree",
    description: "Pick a project to adopt an existing worktree as an agent.",
  },
} as const

function PickerBody() {
  const { spine, bootstrap, newAgentPickerIntent } = useDux()
  // Default to "new" so a missing value (older state, a test that only sets
  // newAgentPickerOpen) still renders the standard create flow.
  const intent = newAgentPickerIntent ?? "new"
  const projects = useMemo(() => spine?.projects ?? [], [spine])
  const sessions = useMemo(() => spine?.sessions ?? [], [spine])
  const providers = bootstrap?.available_providers ?? []

  const [query, setQuery] = useState("")
  // Preselect the first project so Create is reachable in one click in the common
  // single-project case; the provider follows that project's default.
  const [selectedId, setSelectedId] = useState<string | null>(
    () => projects[0]?.id ?? null,
  )
  const [provider, setProvider] = useState<string | null>(
    () => projects[0]?.default_provider ?? null,
  )

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

  const selected: ProjectView | null =
    projects.find((project) => project.id === selectedId) ?? null

  function selectProject(project: ProjectView) {
    setSelectedId(project.id)
    // Reset the provider choice to the newly selected project's default so the
    // chip row always reflects that project's default_provider on switch.
    setProvider(project.default_provider)
  }

  // What clicking a project row does, by intent. "new" just selects it (Create
  // finishes the flow); the from-PR / from-worktree intents close this picker and
  // hand off to that project's dedicated dialog immediately.
  function onProjectRow(project: ProjectView) {
    if (intent === "from_pr") {
      closeNewAgentPicker()
      openCreateAgentFromPr(project.id)
      return
    }
    if (intent === "from_worktree") {
      closeNewAgentPicker()
      openAttachWorktree(project.id)
      return
    }
    selectProject(project)
  }

  function handleCreate() {
    if (!selected) return
    createAgentInProject(selected.id, provider ?? undefined)
  }

  return (
    <>
      <DialogHeader className="p-4 pb-3">
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

        <ScrollArea className="max-h-72 border-t">
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
                // Only the "new" flow has a persistent selection; the guided
                // from-PR / from-worktree flows act on click, so no row is "chosen".
                const isSelected = intent === "new" && project.id === selectedId
                return (
                  <div
                    key={project.id}
                    className={cn(
                      "flex items-center gap-2 rounded-md px-2 transition-colors max-md:min-h-10",
                      isSelected
                        ? "bg-accent text-accent-foreground"
                        : "hover:bg-accent/60",
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
                      <span className="shrink-0 font-mono text-xs text-muted-foreground">
                        {count} {count === 1 ? "agent" : "agents"}
                      </span>
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

        <div className="border-t p-2">
          <button
            type="button"
            onClick={openAddProject}
            className="flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left text-sm text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground max-md:min-h-10"
          >
            <FolderPlus className="size-4 shrink-0" />
            Add a new project…
          </button>
        </div>

        {intent === "new" ? (
        <div className="flex flex-wrap items-center gap-2 border-t p-3">
          {providers.length > 0 ? (
            <>
              <span className="text-sm text-muted-foreground">Provider</span>
              {providers.map((name) => {
                const active = name === provider
                return (
                  <button
                    key={name}
                    type="button"
                    onClick={() => setProvider(name)}
                    aria-pressed={active}
                    className={cn(
                      "rounded-full border px-3 py-1 font-mono text-xs transition-colors max-md:min-h-10",
                      active
                        ? "border-foreground bg-foreground text-background"
                        : "border-border text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {name}
                  </button>
                )
              })}
            </>
          ) : null}
          <Button
            className="ml-auto max-md:min-h-10"
            disabled={!selected}
            onClick={handleCreate}
          >
            Create agent
          </Button>
        </div>
        ) : null}
    </>
  )
}
