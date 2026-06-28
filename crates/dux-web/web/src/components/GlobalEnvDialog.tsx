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
import { closeGlobalEnv, saveGlobalEnv, useDux } from "@/lib/store"

// The form body is a separate component mounted only while the dialog is open.
// It seeds its `useState` from the `env` prop via a lazy initializer, so there
// is no set-state-in-effect to seed the textarea on open.
function GlobalEnvForm({ env }: { env: Record<string, string> }) {
  const [text, setText] = useState(() => envToText(env))

  function handleSave() {
    saveGlobalEnv(parseEnv(text))
    closeGlobalEnv()
  }

  return (
    <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>Global environment</DialogTitle>
        <DialogDescription>
          KEY=VALUE per line. Applied to new agents and terminals unless a
          project overrides the same key.
        </DialogDescription>
      </DialogHeader>
      <Textarea
        placeholder="KEY=VALUE"
        value={text}
        onChange={(e) => setText(e.target.value)}
        className="min-h-48 font-mono"
        autoFocus
      />
      <DialogFooter>
        <Button variant="outline" onClick={closeGlobalEnv}>
          Cancel
        </Button>
        <Button onClick={handleSave}>Save</Button>
      </DialogFooter>
    </DialogContent>
  )
}

export function GlobalEnvDialog() {
  const { bootstrap, globalEnvOpen } = useDux()

  return (
    <Dialog
      open={globalEnvOpen}
      onOpenChange={(o) => {
        if (!o) closeGlobalEnv()
      }}
    >
      {globalEnvOpen && <GlobalEnvForm env={bootstrap?.global_env ?? {}} />}
    </Dialog>
  )
}
