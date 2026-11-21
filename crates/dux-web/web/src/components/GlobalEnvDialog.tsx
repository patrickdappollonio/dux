import { useState } from "react"
import { TriangleAlert } from "lucide-react"
import { FormDialogContent } from "@/components/FormDialogContent"
import { Button } from "@/components/ui/button"
import { Dialog } from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import { useIsMobile } from "@/hooks/use-mobile"
import { envToText, parseEnv } from "@/lib/env"
import {
  closeGlobalEnv,
  dismissGlobalEnvConflict,
  reloadGlobalEnvDraft,
  saveGlobalEnv,
  useDux,
} from "@/lib/store"

// The form body is a separate component mounted only while the dialog is open.
// It seeds its `useState` from the `env` prop via a lazy initializer, so there
// is no set-state-in-effect to seed the textarea on open.
function GlobalEnvForm({
  env,
  conflict,
}: {
  env: Record<string, string>
  conflict: string | null
}) {
  const [text, setText] = useState(() => envToText(env))

  function handleSave() {
    saveGlobalEnv(parseEnv(text))
    closeGlobalEnv()
  }

  // On a phone, focusing the text would raise the keyboard over the dialog.
  const phone = useIsMobile()
  return (
    <FormDialogContent
      phone={phone}
      title="Global environment"
      description="KEY=VALUE per line. Applied to new agents and terminals unless a project overrides the same key."
      onClose={closeGlobalEnv}
      footer={
        <>
          <Button variant="outline" onClick={closeGlobalEnv}>
            Cancel
          </Button>
          <Button onClick={handleSave}>Save</Button>
        </>
      }
    >
      <Textarea
        placeholder="KEY=VALUE"
        value={text}
        onChange={(e) => setText(e.target.value)}
        className="min-h-48 font-mono max-md:flex-1"
        autoFocus={conflict === null && !phone}
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
            <Button variant="outline" autoFocus onClick={dismissGlobalEnvConflict} className="max-md:min-h-10 pointer-coarse:min-h-10">
              Keep editing
            </Button>
            <Button variant="destructive" onClick={() => void reloadGlobalEnvDraft()} className="max-md:min-h-10 pointer-coarse:min-h-10">
              Reload the environment
            </Button>
          </div>
        </div>
      ) : null}
    </FormDialogContent>
  )
}

export function GlobalEnvDialog() {
  const { bootstrap, globalEnvOpen, globalEnvDraft, globalEnvConflict, globalEnvEpoch } =
    useDux()

  return (
    <Dialog
      open={globalEnvOpen}
      onOpenChange={(o) => {
        if (!o) closeGlobalEnv()
      }}
    >
      {globalEnvOpen && (
        <GlobalEnvForm
          key={globalEnvEpoch}
          env={globalEnvDraft ?? bootstrap?.global_env ?? {}}
          conflict={globalEnvConflict}
        />
      )}
    </Dialog>
  )
}
