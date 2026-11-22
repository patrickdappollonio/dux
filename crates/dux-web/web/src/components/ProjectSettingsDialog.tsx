import { useState } from "react"

import { Modal, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import { envToText, parseEnv } from "@/lib/env"
import { chip, prose } from "@/lib/prose"
import { closeProjectSettings, updateProjectSettings, useDux } from "@/lib/store"
import type { PatchProjectBody } from "@/lib/projectsApi"
import type { ProjectView } from "@/lib/types"

export function ProjectSettingsDialog() {
  const { projectSettingsTarget } = useDux()
  const open = projectSettingsTarget !== null
  const life = useModalLifetime(open)
  // Keyed by the open, so every control starts from the project's settings.
  return life.mounted ? (
    <ProjectSettingsForm key={life.epoch} open={open} onClosed={life.onClosed} />
  ) : null
}

interface Draft {
  provider: string
  autoReopen: string
  startup: string
  envText: string
}

// One tri-state PATCH body: only the fields that changed (the server leaves an
// omitted field untouched), with `null` clearing a scalar back to the
// inherited default.
function settingsPatch(project: ProjectView, draft: Draft): PatchProjectBody {
  const patch: PatchProjectBody = {}

  const newProvider = draft.provider === "" ? null : draft.provider
  if (newProvider !== (project.explicit_default_provider ?? null)) {
    patch.provider = newProvider
  }

  const newAutoReopen =
    draft.autoReopen === "inherit" ? null : draft.autoReopen === "on" ? true : false
  if (newAutoReopen !== (project.auto_reopen_agents ?? null)) {
    patch.auto_reopen_agents = newAutoReopen
  }

  const newStartup = draft.startup.trim() === "" ? null : draft.startup
  if (newStartup !== (project.startup_command ?? null)) {
    patch.startup_command = newStartup
  }

  const env = parseEnv(draft.envText)
  if (JSON.stringify(env) !== JSON.stringify(project.env)) {
    patch.env = env
  }
  return patch
}

function ProjectSettingsForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { spine, bootstrap, projectSettingsTarget } = useDux()
  // The project this open is about, kept while the close animates.
  const [targetId] = useState(projectSettingsTarget)
  const project = spine?.projects.find((p) => p.id === targetId)
  const providers = bootstrap?.available_providers ?? []
  // "" means "inherit the global default provider".
  const [provider, setProvider] = useState<string>(
    () => project?.explicit_default_provider ?? "",
  )
  const [autoReopen, setAutoReopen] = useState<string>(() =>
    project?.auto_reopen_agents == null
      ? "inherit"
      : project.auto_reopen_agents
        ? "on"
        : "off",
  )
  const [startup, setStartup] = useState(() => project?.startup_command ?? "")
  const [envText, setEnvText] = useState(() => envToText(project?.env ?? {}))
  const [saving, setSaving] = useState(false)
  const patch =
    project === undefined ? {} : settingsPatch(project, { provider, autoReopen, startup, envText })
  const changes = Object.keys(patch).length

  async function handleSave() {
    if (project === undefined || saving) return
    setSaving(true)
    try {
      // The PATCH is not atomic across its fields, so a rejection (toasted)
      // keeps the dialog open over what may be a partly applied change.
      if (await updateProjectSettings(project.id, patch)) closeProjectSettings()
    } finally {
      setSaving(false)
    }
  }

  const projectName = project?.name ?? ""
  return (
    <Modal
      kind="form"
      open={open}
      // A project removed while the dialog is open closes it; a rejected save,
      // by contrast, deliberately keeps it open.
      vanished={open && project === undefined}
      onClose={closeProjectSettings}
      onClosed={onClosed}
      size="lg"
      title={
        <>
          Project settings: <InlineCode>{projectName}</InlineCode>
        </>
      }
      description="Per-project overrides. Leave a field on the inherited default to fall back to the global configuration."
      submit={{
        label: "Save",
        tone: "primary",
        onSelect: () => void handleSave(),
        busy: saving,
        disabledReason: changes === 0 ? "Nothing to save yet: no setting has changed." : null,
      }}
      unsavedEdits={{
        changes,
        holder: "The project settings have",
        leaves: prose`the settings of project ${chip(projectName)}`,
      }}
    >
      <div className="grid gap-4">
        <div className="grid gap-2">
          <label className="text-sm font-medium">Default provider</label>
          <Select
            value={provider}
            onValueChange={(value) => setProvider(value ?? "")}
          >
            <SelectTrigger className="w-full">
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
            <SelectTrigger className="w-full">
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
          <Textarea
            value={startup}
            onChange={(e) => setStartup(e.target.value)}
            placeholder="npm run dev"
            className="min-h-16 font-mono"
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
    </Modal>
  )
}
