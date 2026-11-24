import { useState } from "react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { FormGroup } from "@/components/ui/form-group"
import { Textarea } from "@/components/ui/textarea"
import { useIsMobile } from "@/hooks/use-mobile"
import { envEdit, envLeftOutNote, envToText } from "@/lib/env"
import { chip, prose } from "@/lib/prose"
import { closeAgentEnv, updateProjectSettings, useDux } from "@/lib/store"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"

// Edit environment variables from an agent's menu. Env is project-scoped, so this
// edits the agent's PROJECT env, layered over the global env, and the copy says so.
export function AgentEnvDialog() {
  const { agentEnvTarget } = useDux()
  const open = agentEnvTarget !== null
  const life = useModalLifetime(open)
  // Keyed by the open, so the text starts from the project's env each time.
  return life.mounted ? (
    <AgentEnvForm key={life.epoch} open={open} onClosed={life.onClosed} />
  ) : null
}

function AgentEnvForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { spine, agentEnvTarget } = useDux()
  // The agent this open is about, kept while the close animates.
  const [targetId] = useState(agentEnvTarget)
  // The agent and its project as they were when the close began, kept while
  // the close animates, so the settings a save just applied do not read as
  // unchanged.
  const session = useHeldWhileClosed(
    open,
    spine?.sessions.find((s) => s.id === targetId),
  )
  const project = useHeldWhileClosed(
    open,
    spine?.projects.find(
      (p) => session !== undefined && p.id === workspaceProjectId(session.workspace),
    ),
  )
  const [opened] = useState(() => envToText(project?.env ?? {}))
  const [text, setText] = useState(opened)
  const [savingNow, setSaving] = useState(false)
  const saving = useHeldWhileClosed(open, savingNow)
  const edit = envEdit(project?.env ?? {}, opened, text)
  const leftOutNote = envLeftOutNote(edit.leftOut)
  // On a phone the text takes the height the covering dialog leaves.
  const phone = useIsMobile()

  async function handleSave() {
    if (project === undefined || edit.changed === 0 || saving) return
    setSaving(true)
    try {
      if (await updateProjectSettings(project.id, { env: edit.env }, "environment")) {
        closeAgentEnv()
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
      onClose={closeAgentEnv}
      onClosed={onClosed}
      title={
        <>
          Environment: <InlineCode>{session ? sessionLabel(session) : ""}</InlineCode>
        </>
      }
      description={
        <>
          KEY=VALUE per line, applied to every agent and terminal in project{" "}
          <InlineCode>{projectName}</InlineCode> (layered over the global env). This applies
          to the whole project, not just this agent.
        </>
      }
      busy={saving}
      submit={{
        label: "Save",
        tone: "primary",
        onSelect: () => void handleSave(),
        busy: saving,
        disabledReason: edit.changed === 0 ? "Nothing to save yet: no variable has changed." : null,
      }}
      unsavedEdits={{
        changes: edit.edits,
        holder: "The environment has",
        leaves: prose`the environment of project ${chip(projectName)}`,
      }}
    >
      <FormGroup
        layout="stacked"
        height={phone ? "fill" : "content"}
        control="single"
        spokenName="Environment variables"
        notes={[{ words: leftOutNote }]}
      >
        {(field) => (
          <Textarea
            {...field}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="KEY=VALUE"
            className="min-h-48 font-mono max-md:flex-1"
          />
        )}
      </FormGroup>
    </Modal>
  )
}
