import { useLayoutEffect, useRef, useState } from "react"
import { TriangleAlert } from "lucide-react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"
import { envEdit, envLeftOutNote, envToText } from "@/lib/env"
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
  const { bootstrap, globalEnvDraft, globalEnvConflict, globalEnvEpoch } = useDux()
  // The refusal stays on screen while the close animates.
  const conflict = useHeldWhileClosed(open, globalEnvConflict)
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
  // Opened by a refused save rather than by the person, so Back must not be
  // what answers it.
  const [historyEntry] = useState(globalEnvDraft === null)
  const keepEditing = useRef<HTMLButtonElement | null>(null)
  const field = useRef<HTMLTextAreaElement | null>(null)
  const coarse = useIsCoarsePointer()
  const edit = envEdit(seeded.saved, seeded.text, text)
  const leftOutNote = envLeftOutNote(edit.leftOut)

  // A reload reseeds the text and takes the Reload button away with its
  // banner, so focus is placed again as an open places it: the text under a
  // mouse, and the dialog itself under a finger, where the text would raise
  // the keyboard.
  const placedFor = useRef(seeded.epoch)
  useLayoutEffect(() => {
    if (placedFor.current === seeded.epoch) return
    placedFor.current = seeded.epoch
    const target = coarse
      ? field.current?.closest<HTMLElement>('[data-slot="dialog-content"]')
      : field.current
    target?.focus({ preventScroll: true })
  }, [seeded.epoch, coarse])

  function handleSave() {
    if (edit.changed === 0) return
    saveGlobalEnv(edit.env)
    closeGlobalEnv()
  }

  return (
    <Modal
      kind="form"
      open={open}
      onClose={closeGlobalEnv}
      onClosed={onClosed}
      historyEntry={historyEntry}
      title="Global environment"
      description="KEY=VALUE per line. Applied to new agents and terminals unless a project overrides the same key."
      // A refused save reopens the dialog with Keep editing focused, since
      // the other answer throws these edits away.
      initialFocus={conflict ? keepEditing : undefined}
      submit={{
        label: "Save",
        tone: "primary",
        onSelect: handleSave,
        disabledReason: edit.changed === 0 ? "Nothing to save yet: no variable has changed." : null,
      }}
      unsavedEdits={{
        changes: edit.edits,
        holder: "The global environment has",
        leaves: ["the global environment"],
      }}
    >
      <Textarea
        ref={field}
        placeholder="KEY=VALUE"
        value={text}
        onChange={(e) => setText(e.target.value)}
        className="min-h-48 font-mono max-md:flex-1"
      />
      {leftOutNote ? <p className="text-sm text-muted-foreground">{leftOutNote}</p> : null}
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
