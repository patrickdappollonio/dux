import { useState } from "react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { Textarea } from "@/components/ui/textarea"
import { chip, prose } from "@/lib/prose"
import {
  closeAgentStartupCommand,
  updateProjectSettings,
  useDux,
} from "@/lib/store"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"

// Edit the startup command from an agent's menu. It is project-scoped, so this
// edits the agent's PROJECT and the copy says so.
export function AgentStartupCommandDialog() {
  const { agentStartupCommandTarget } = useDux()
  const open = agentStartupCommandTarget !== null
  const life = useModalLifetime(open)
  // Keyed by the open, so the text starts from the project's command each time.
  return life.mounted ? (
    <AgentStartupCommandForm key={life.epoch} open={open} onClosed={life.onClosed} />
  ) : null
}

function AgentStartupCommandForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { spine, agentStartupCommandTarget } = useDux()
  // The agent this open is about, kept while the close animates.
  const [targetId] = useState(agentStartupCommandTarget)
  // The agent and its project as they were when the close began, kept while
  // the close animates, so the settings a save just applied do not read as
  // unchanged.
  const session = useHeldWhileClosed(
    open,
    spine?.sessions.find((s) => s.id === targetId),
  )
  // A standalone agent belongs to no project, so the lookup must not happen at
  // all for one rather than miss.
  const project = useHeldWhileClosed(
    open,
    spine?.projects.find(
      (p) => session !== undefined && p.id === workspaceProjectId(session.workspace),
    ),
  )
  const [startup, setStartup] = useState(() => project?.startup_command ?? "")
  const next = startup.trim() === "" ? null : startup
  const changed = project !== undefined && next !== (project.startup_command ?? null)
  const [savingNow, setSaving] = useState(false)
  const saving = useHeldWhileClosed(open, savingNow)

  async function handleSave() {
    if (project === undefined || !changed || saving) return
    setSaving(true)
    try {
      if (await updateProjectSettings(project.id, { startup_command: next }, "startup command")) {
        closeAgentStartupCommand()
      }
    } finally {
      setSaving(false)
    }
  }

  const projectName = project?.name ?? ""
  return (
    <Modal
      kind="form"
      open={open}
      // The agent or its project removed while the dialog is open closes it.
      vanished={open && (session === undefined || project === undefined)}
      onClose={closeAgentStartupCommand}
      onClosed={onClosed}
      title={
        <>
          Startup command: <InlineCode>{session ? sessionLabel(session) : ""}</InlineCode>
        </>
      }
      description={
        <>
          Runs after each agent or terminal launches in project{" "}
          <InlineCode>{projectName}</InlineCode>. This applies to every agent in the project,
          not just this one. Leave empty to clear it.
        </>
      }
      busy={saving}
      submit={{
        label: "Save",
        tone: "primary",
        onSelect: () => void handleSave(),
        busy: saving,
        disabledReason: changed ? null : "Nothing to save yet: the startup command has not changed.",
      }}
      unsavedEdits={{
        changes: changed ? 1 : 0,
        holder: "The startup command has",
        leaves: prose`the startup command of project ${chip(projectName)}`,
      }}
    >
      <Textarea
        value={startup}
        onChange={(e) => setStartup(e.target.value)}
        onKeyDown={(e) => {
          // Plain Enter inserts a newline (a startup command can be multi-line);
          // Cmd/Ctrl-Enter saves, matching the common textarea submit convention.
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault()
            void handleSave()
          }
        }}
        placeholder="npm run dev"
        className="min-h-16 font-mono"
      />
    </Modal>
  )
}
