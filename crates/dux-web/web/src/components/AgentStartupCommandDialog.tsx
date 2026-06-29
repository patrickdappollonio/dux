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
import { Input } from "@/components/ui/input"
import {
  closeAgentStartupCommand,
  updateProjectSettings,
  useDux,
} from "@/lib/store"
import type { ProjectView, SessionView } from "@/lib/types"

// Edit the startup command from an agent's menu. Startup command is
// project-scoped in dux (there is no per-agent startup command), so this edits
// the agent's PROJECT — the dialog makes that explicit. The form body is mounted
// only while open and a project resolves; its state seeds lazily from the project
// (no set-state-in-effect), mirroring ProjectSettingsDialog.
function AgentStartupCommandForm({
  session,
  project,
}: {
  session: SessionView
  project: ProjectView
}) {
  const [startup, setStartup] = useState(() => project.startup_command ?? "")
  const agentName = session.title || session.branch_name

  async function handleSave() {
    const next = startup.trim() === "" ? null : startup
    // No-op when unchanged; `updateProjectSettings` skips the request on an empty
    // patch, so saving without a change just closes.
    const patch =
      next === (project.startup_command ?? null) ? {} : { startup_command: next }
    if (await updateProjectSettings(project.id, patch)) closeAgentStartupCommand()
  }

  return (
    <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Startup command — {agentName}</DialogTitle>
        <DialogDescription>
          Runs after each agent or terminal launches in project{" "}
          <span className="font-medium">{project.name}</span>. This applies to
          every agent in the project, not just this one. Leave empty to clear it.
        </DialogDescription>
      </DialogHeader>
      <Input
        value={startup}
        onChange={(e) => setStartup(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault()
            void handleSave()
          }
        }}
        placeholder="npm run dev"
        className="font-mono"
        autoFocus
      />
      <DialogFooter>
        <Button variant="outline" onClick={closeAgentStartupCommand}>
          Cancel
        </Button>
        <Button onClick={() => void handleSave()}>Save</Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function AgentStartupCommandDialog() {
  const { spine, agentStartupCommandTarget } = useDux()
  const session = spine?.sessions.find((s) => s.id === agentStartupCommandTarget)
  const project = spine?.projects.find((p) => p.id === session?.project_id)
  const open = agentStartupCommandTarget !== null

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeAgentStartupCommand()
      }}
    >
      {open && session && project && (
        <AgentStartupCommandForm session={session} project={project} />
      )}
    </Dialog>
  )
}
