import { useState } from "react"

import { Modal, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { Textarea } from "@/components/ui/textarea"
import { envChangeCount, envToText, parseEnv } from "@/lib/env"
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
  const session = spine?.sessions.find((s) => s.id === targetId)
  const project = spine?.projects.find(
    (p) => session !== undefined && p.id === workspaceProjectId(session.workspace),
  )
  const [text, setText] = useState(() => envToText(project?.env ?? {}))
  const saved = project?.env ?? {}

  async function handleSave() {
    if (project === undefined) return
    const env = parseEnv(text)
    // Unchanged is an empty patch: no request, a nothing-to-save notice, close.
    const patch =
      JSON.stringify(env) === JSON.stringify(project.env) ? {} : { env }
    if (await updateProjectSettings(project.id, patch, "environment")) closeAgentEnv()
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
      submit={{ label: "Save", tone: "primary", onSelect: () => void handleSave() }}
      unsavedEdits={{
        changes: envChangeCount(saved, parseEnv(text)),
        holder: "The environment has",
        leaves: prose`the environment of project ${chip(projectName)}`,
      }}
    >
      <Textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="KEY=VALUE"
        className="min-h-48 font-mono max-md:flex-1"
      />
    </Modal>
  )
}
