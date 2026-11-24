import { useLayoutEffect, useRef, useState } from "react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { StaleSaveNotice } from "@/components/StaleSaveNotice"
import { FormGroup } from "@/components/ui/form-group"
import { Textarea } from "@/components/ui/textarea"
import { useIsMobile } from "@/hooks/use-mobile"
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
  // On a phone the text takes the height the covering dialog leaves.
  const phone = useIsMobile()

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
      <FormGroup
        layout="stacked"
        height={phone ? "fill" : "content"}
        control="single"
        spokenName="Environment variables"
        notes={[{ words: leftOutNote }]}
      >
        {(wiring) => (
          <Textarea
            {...wiring}
            ref={field}
            placeholder="KEY=VALUE"
            value={text}
            onChange={(e) => setText(e.target.value)}
            className="min-h-48 font-mono max-md:flex-1"
          />
        )}
      </FormGroup>
      {conflict ? (
        // The table changed after the dialog read it and nothing was saved.
        // Reloading throws these edits away, so "Keep editing" has focus.
        <StaleSaveNotice
          message={conflict}
          reloadLabel="Reload the environment"
          onKeepEditing={dismissGlobalEnvConflict}
          onReload={() => void reloadGlobalEnvDraft()}
          keepEditingRef={keepEditing}
        />
      ) : null}
    </Modal>
  )
}
