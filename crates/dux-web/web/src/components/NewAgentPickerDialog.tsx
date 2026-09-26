import { useMemo, useState } from "react"

import { ProjectList, type ProjectListRow } from "@/components/ProjectList"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { Dialog, DialogContent } from "@/components/ui/dialog"
import {
  closeNewAgentPicker,
  dismissNewAgentPicker,
  openAttachWorktree,
  openCreateAgent,
  openCreateAgentFromPr,
  useDux,
} from "@/lib/store"
import {
  applyFrozenOrder,
  orderProjectsByRecency,
  projectAgentCounts,
} from "@/lib/projectOrder"
import { formatRegularCount } from "@/lib/formatRegularCount"

// The New-agent picker: the home for agent creation and every project action,
// since the flat list has no project headers. A searchable list of all
// projects, agent-less ones included because this is where their first agent is
// created, each keeping its full `ProjectMenuItems` menu. A row click in the
// "new" intent opens the shared create-agent name dialog for that project.
// Provider is not chosen at creation on the web: an agent launches on the
// project's default_provider and is retargeted from the agent menu.
export function NewAgentPickerDialog() {
  const { newAgentPickerOpen } = useDux()
  return (
    <Dialog
      open={newAgentPickerOpen}
      onOpenChange={(open) => {
        if (!open) dismissNewAgentPicker()
      }}
    >
      {/* Flex column, deliberately not overflow-hidden: the popup's base max-h
          caps it to the dynamic viewport, and when the soft keyboard shrinks
          100dvh the list shrinks with it as a min-h-0 flex child rather than
          the popup clipping with nothing left to scroll. The base
          overflow-y-auto stays as the last-resort scroll. */}
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
  const sessions = useMemo(() => spine?.sessions ?? [], [spine])
  // Narrowed to a candidate set when a pull-request reference matched several
  // projects: showing every project there would bury the two that are actually
  // checkouts of that repository.
  const candidates = useMemo(() => {
    const all = spine?.projects ?? []
    if (!newAgentPickerOnlyIds) return all
    const only = new Set(newAgentPickerOnlyIds)
    return all.filter((project) => only.has(project.id))
  }, [spine, newAgentPickerOnlyIds])
  // The recency order is snapshotted at open, the way the terminal UI snapshots
  // its chooser: a list that re-sorts under the pointer lands a click on the
  // wrong project. This body mounts only while the dialog is open, so a mount is
  // an open, and the narrowed set is read once here too, since a pull-request
  // reference is not expected to change while its picker is up.
  const [frozenIds] = useState(() =>
    orderProjectsByRecency(candidates, sessions).map((project) => project.id),
  )
  // The rows are the live spine's own objects, so names and agent counts keep
  // updating inside the frozen order.
  const projects = useMemo(
    () => applyFrozenOrder(frozenIds, candidates),
    [frozenIds, candidates],
  )

  // Agent counts per project, derived by cross-referencing sessions (the project
  // record carries no count of its own).
  const agentCounts = useMemo(() => projectAgentCounts(sessions), [sessions])

  // What clicking a project row does, by intent. Every intent closes this picker
  // and hands off to that project's dedicated dialog: "new" opens the shared
  // create-agent name dialog (honoring the pet-name/copy-changes config), and the
  // from-PR / from-worktree intents open their own dialogs.
  function onProjectRow(projectId: string) {
    if (intent === "from_pr") {
      closeNewAgentPicker()
      openCreateAgentFromPr(projectId)
      return
    }
    if (intent === "from_worktree") {
      closeNewAgentPicker()
      // `true` marks the drill-down: the Worktrees dialog then offers a Back
      // control that returns to this list, instead of Cancel being the only
      // way out of a project that turned out to have nothing in it.
      openAttachWorktree(projectId, true)
      return
    }
    closeNewAgentPicker()
    openCreateAgent(projectId)
  }

  const rows: ProjectListRow[] = projects.map((project) => {
    const count = agentCounts.get(project.id) ?? 0
    // In the worktree intent the row is a doorway into that project's worktree
    // list, so it is labelled with what is behind the door. An empty project
    // reads "none" and stays clickable, because disabling it would give no
    // reason and read as broken; a count that has not arrived shows no label at
    // all rather than a misleading zero.
    const worktreeCount = projectWorktreeCounts?.[project.id]
    const label =
      intent === "from_worktree"
        ? projectWorktreeCounts === undefined || projectWorktreeCounts === null
          ? null
          : (worktreeCount ?? 0) === 0
            ? "none"
            : formatRegularCount(worktreeCount ?? 0, "worktree")
        : formatRegularCount(count, "agent")
    return { id: project.id, name: project.name, label }
  })

  return (
    <ProjectList
      title={INTENT_COPY[intent].title}
      description={INTENT_COPY[intent].description}
      rows={rows}
      onPick={onProjectRow}
      // New agent… and New terminal take the user somewhere else, so the
      // picker closes first rather than staying open over where they land.
      menu={(id) => <ProjectMenuItems id={id} onLeave={closeNewAgentPicker} />}
    />
  )
}
