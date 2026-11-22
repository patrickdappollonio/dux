import { FolderPlus } from "lucide-react"
import { useMemo, useState } from "react"

import { Modal, useModalLifetime, type ModalDoor } from "@/components/modal"
import { ProjectRows } from "@/components/ProjectList"
import { useProjectSearch, type ProjectListRow } from "@/components/projectSearch"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import {
  closeNewAgentPicker,
  dismissNewAgentPicker,
  openAddProject,
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
import { STANDALONE_AGENT_CREATION } from "@/lib/creationMenus"
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
  const life = useModalLifetime(newAgentPickerOpen)
  // Keyed by the open, so each one starts with a fresh search and order.
  return life.mounted ? (
    <Picker key={life.epoch} open={newAgentPickerOpen} onClosed={life.onClosed} />
  ) : null
}

// Per-intent copy + the action a project row fires. "new" keeps the pick-provider-
// then-Create flow; the other two are guided "pick a project" flows that hand off
// to the existing from-PR / attach-worktree dialogs.
const INTENT_COPY = {
  new: {
    title: "New agent",
    description:
      "An agent is an AI coding session with its own branch and working copy. Run several at once on one project without them touching each other's files.",
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

function Picker({ open, onClosed }: { open: boolean; onClosed: () => void }) {
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
  // wrong project. This component is keyed by the open, so a mount is an open,
  // and the narrowed set is read once here too, since a pull-request reference
  // is not expected to change while its picker is up.
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

  const search = useProjectSearch(rows, onProjectRow)
  // A standalone agent needs no project, so only the plain intent offers it.
  // It opens a different dialog, so the picker closes first.
  const addProject: ModalDoor = {
    label: "Add a new project…",
    icon: FolderPlus,
    onSelect: openAddProject,
  }
  const addStandalone: ModalDoor = {
    label: "Add standalone agent…",
    icon: STANDALONE_AGENT_CREATION.icon,
    onSelect: () => {
      closeNewAgentPicker()
      STANDALONE_AGENT_CREATION.run()
    },
  }
  return (
    <Modal
      kind="picker"
      open={open}
      onClose={dismissNewAgentPicker}
      onClosed={onClosed}
      size="lg"
      title={INTENT_COPY[intent].title}
      description={INTENT_COPY[intent].description}
      search={{
        value: search.query,
        onChange: search.setQuery,
        label: "Search projects",
        fieldProps: search.pick.fieldProps,
      }}
      steadyHeight
      doors={intent === "new" ? [addProject, addStandalone] : [addProject]}
    >
      <ProjectRows
        search={search}
        onPick={onProjectRow}
        // New agent… and New terminal take the user somewhere else, so the
        // picker closes first rather than staying open over where they land.
        menu={(id) => <ProjectMenuItems id={id} onLeave={closeNewAgentPicker} />}
      />
    </Modal>
  )
}
