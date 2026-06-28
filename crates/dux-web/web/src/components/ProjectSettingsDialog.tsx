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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import { envToText, parseEnv } from "@/lib/env"
import { closeProjectSettings, updateProjectSettings, useDux } from "@/lib/store"
import type { PatchProjectBody } from "@/lib/projectsApi"
import type { ProjectView } from "@/lib/types"

// The form body is mounted only while the dialog is open and a project resolves.
// Each local state seeds from the project prop via a lazy initializer so there
// is no set-state-in-effect to seed the controls on open.
function ProjectSettingsForm({
  project,
  providers,
}: {
  project: ProjectView
  providers: string[]
}) {
  // "" means "inherit the global default provider".
  const [provider, setProvider] = useState<string>(
    () => project.explicit_default_provider ?? "",
  )
  const [autoReopen, setAutoReopen] = useState<string>(() =>
    project.auto_reopen_agents === null
      ? "inherit"
      : project.auto_reopen_agents
        ? "on"
        : "off",
  )
  const [startup, setStartup] = useState(() => project.startup_command ?? "")
  const [envText, setEnvText] = useState(() => envToText(project.env))

  async function handleSave() {
    // Build one tri-state PATCH body: include only the fields that changed (the
    // server leaves an omitted field untouched), with `null` clearing a scalar
    // back to the inherited default. `updateProjectSettings` no-ops on an empty
    // patch, so saving with nothing changed sends no request.
    const patch: PatchProjectBody = {}

    const newProvider = provider === "" ? null : provider
    if (newProvider !== (project.explicit_default_provider ?? null)) {
      patch.provider = newProvider
    }

    const newAutoReopen =
      autoReopen === "inherit" ? null : autoReopen === "on" ? true : false
    if (newAutoReopen !== (project.auto_reopen_agents ?? null)) {
      patch.auto_reopen_agents = newAutoReopen
    }

    const newStartup = startup.trim() === "" ? null : startup
    if (newStartup !== (project.startup_command ?? null)) {
      patch.startup_command = newStartup
    }

    const env = parseEnv(envText)
    if (JSON.stringify(env) !== JSON.stringify(project.env)) {
      patch.env = env
    }

    // Close only once the PATCH succeeds. The PATCH is not atomic across its
    // independent fields, so on a rejection we keep the dialog open (the error is
    // toasted) rather than dismissing it over a partially-applied change.
    if (await updateProjectSettings(project.id, patch)) closeProjectSettings()
  }

  return (
    <DialogContent showCloseButton={false} className="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Project settings — {project.name}</DialogTitle>
        <DialogDescription>
          Per-project overrides. Leave a field on the inherited default to fall
          back to the global configuration.
        </DialogDescription>
      </DialogHeader>

      <div className="grid gap-4">
        <div className="grid gap-2">
          <label className="text-sm font-medium">Default provider</label>
          <Select
            value={provider}
            onValueChange={(value) => setProvider(value ?? "")}
          >
            <SelectTrigger className="w-full max-md:min-h-11">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="">Inherit global default</SelectItem>
              {providers.map((p) => (
                <SelectItem key={p} value={p}>
                  {p}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="grid gap-2">
          <label className="text-sm font-medium">Auto-reopen agents</label>
          <Select
            value={autoReopen}
            onValueChange={(value) => setAutoReopen(value ?? "inherit")}
          >
            <SelectTrigger className="w-full max-md:min-h-11">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="inherit">Inherit global default</SelectItem>
              <SelectItem value="on">On</SelectItem>
              <SelectItem value="off">Off</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="grid gap-2">
          <label className="text-sm font-medium">Startup command</label>
          <Input
            value={startup}
            onChange={(e) => setStartup(e.target.value)}
            placeholder="npm run dev"
            className="font-mono"
          />
        </div>

        <div className="grid gap-2">
          <label className="text-sm font-medium">Environment</label>
          <Textarea
            value={envText}
            onChange={(e) => setEnvText(e.target.value)}
            placeholder="KEY=VALUE"
            className="min-h-32 font-mono"
          />
        </div>
      </div>

      <DialogFooter>
        <Button variant="outline" onClick={closeProjectSettings}>
          Cancel
        </Button>
        <Button onClick={handleSave}>Save</Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function ProjectSettingsDialog() {
  const { spine, bootstrap, projectSettingsTarget } = useDux()
  const open = projectSettingsTarget !== null
  const project = spine?.projects.find(
    (p) => p.id === projectSettingsTarget,
  )

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeProjectSettings()
      }}
    >
      {open && project && (
        <ProjectSettingsForm
          project={project}
          providers={bootstrap?.available_providers ?? []}
        />
      )}
    </Dialog>
  )
}
