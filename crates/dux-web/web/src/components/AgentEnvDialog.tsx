import { useState } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import { envToText, parseEnv } from "@/lib/env"
import { closeAgentEnv, updateProjectSettings, useDux } from "@/lib/store"
import type { ProjectView, SessionView } from "@/lib/types"

// Edit environment variables from an agent's menu. Env is project-scoped in dux
// (there is no per-agent env), so this edits the agent's PROJECT env — applied to
// every agent and terminal in the project, layered over the global env. The
// dialog makes that scope explicit. Mounted only while open and a project
// resolves; state seeds lazily from the project (no set-state-in-effect),
// mirroring GlobalEnvDialog / ProjectSettingsDialog.
function AgentEnvForm({
  session,
  project,
}: {
  session: SessionView
  project: ProjectView
}) {
  const [text, setText] = useState(() => envToText(project.env))
  const agentName = session.title || session.branch_name

  async function handleSave() {
    const env = parseEnv(text)
    // No-op when unchanged (skips the request and just closes).
    const patch =
      JSON.stringify(env) === JSON.stringify(project.env) ? {} : { env }
    if (await updateProjectSettings(project.id, patch)) closeAgentEnv()
  }

  return (
    <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Environment — {agentName}</DialogTitle>
        <DialogDescription>
          KEY=VALUE per line, applied to every agent and terminal in project{" "}
          <span className="font-medium">{project.name}</span> (layered over the
          global env). This applies to the whole project, not just this agent.
        </DialogDescription>
      </DialogHeader>
      <Textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="KEY=VALUE"
        className="min-h-48 font-mono"
        autoFocus
      />
      <DialogFooter>
        <Button variant="outline" onClick={closeAgentEnv}>
          Cancel
        </Button>
        <Button onClick={() => void handleSave()}>Save</Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function AgentEnvDialog() {
  const { spine, agentEnvTarget } = useDux()
  const session = spine?.sessions.find((s) => s.id === agentEnvTarget)
  const project = spine?.projects.find((p) => p.id === session?.project_id)
  const open = agentEnvTarget !== null

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeAgentEnv()
      }}
    >
      {open && session && project && (
        <AgentEnvForm session={session} project={project} />
      )}
    </Dialog>
  )
}
