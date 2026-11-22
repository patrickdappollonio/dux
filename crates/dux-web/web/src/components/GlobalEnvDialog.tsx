import { useRef, useState } from "react"
import { TriangleAlert } from "lucide-react"

import { Modal, useModalLifetime } from "@/components/modal"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import { envChangeCount, envToText, parseEnv } from "@/lib/env"
import {
  closeGlobalEnv,
  dismissGlobalEnvConflict,
  reloadGlobalEnvDraft,
  saveGlobalEnv,
  useDux,
} from "@/lib/store"

export function GlobalEnvDialog() {
  const { globalEnvOpen } = useDux()
  const life = useModalLifetime(globalEnvOpen)
  return life.mounted ? (
    <GlobalEnvForm key={life.epoch} open={globalEnvOpen} onClosed={life.onClosed} />
  ) : null
}

function GlobalEnvForm({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const { bootstrap, globalEnvDraft, globalEnvConflict: conflict, globalEnvEpoch } = useDux()
  // Reseeded whenever the store bumps the epoch: a refused save reopening it
  // with its edits, or a reload from the server's table.
  const seed = () => ({
    epoch: globalEnvEpoch,
    text: envToText(globalEnvDraft ?? bootstrap?.global_env ?? {}),
    saved: bootstrap?.global_env ?? {},
  })
  const [seeded, setSeeded] = useState(seed)
  const [text, setText] = useState(seeded.text)
  if (seeded.epoch !== globalEnvEpoch) {
    const next = seed()
    setSeeded(next)
    setText(next.text)
  }
  const keepEditing = useRef<HTMLButtonElement | null>(null)

  function handleSave() {
    saveGlobalEnv(parseEnv(text))
    closeGlobalEnv()
  }

  return (
    <Modal
      kind="form"
      open={open}
      onClose={closeGlobalEnv}
      onClosed={onClosed}
      title="Global environment"
      description="KEY=VALUE per line. Applied to new agents and terminals unless a project overrides the same key."
      // A refused save reopens the dialog with Keep editing focused, since
      // the other answer throws these edits away.
      initialFocus={conflict ? keepEditing : undefined}
      submit={{ label: "Save", tone: "primary", onSelect: handleSave }}
      unsavedEdits={{
        changes: envChangeCount(seeded.saved, parseEnv(text)),
        holder: "The global environment has",
        leaves: ["the global environment"],
      }}
    >
      <Textarea
        placeholder="KEY=VALUE"
        value={text}
        onChange={(e) => setText(e.target.value)}
        className="min-h-48 font-mono max-md:flex-1"
      />
      {conflict ? (
        // The table changed after the dialog read it and nothing was saved.
        // Reloading throws these edits away, so "Keep editing" has focus.
        <div
          role="alert"
          className="flex flex-col gap-3 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm"
        >
          <div className="flex items-start gap-2">
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden />
            <span>{conflict}</span>
          </div>
          <div className="flex justify-end gap-3">
            <Button
              ref={keepEditing}
              variant="outline"
              autoFocus
              onClick={dismissGlobalEnvConflict}
              className="max-md:min-h-10 pointer-coarse:min-h-10"
            >
              Keep editing
            </Button>
            <Button
              variant="destructive"
              onClick={() => void reloadGlobalEnvDraft()}
              className="max-md:min-h-10 pointer-coarse:min-h-10"
            >
              Reload the environment
            </Button>
          </div>
        </div>
      ) : null}
    </Modal>
  )
}
