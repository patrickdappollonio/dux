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
import { closeGlobalEnv, saveGlobalEnv, useDux } from "@/lib/store"

// Render the current env as sorted `KEY=VALUE` lines for the textarea.
function envToText(env: Record<string, string>): string {
  return Object.entries(env)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([k, v]) => `${k}=${v}`)
    .join("\n")
}

// Parse the textarea back to an object. Blank lines and `#` comments are
// skipped; the first `=` splits key and value so values may themselves contain
// `=`.
function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {}
  for (const raw of text.split("\n")) {
    const line = raw.trim()
    if (line === "" || line.startsWith("#")) continue
    const eq = line.indexOf("=")
    if (eq <= 0) continue // skip lines with no key
    const key = line.slice(0, eq).trim()
    const value = line.slice(eq + 1) // keep value as-is (may contain '=')
    if (key) env[key] = value
  }
  return env
}

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
  const { viewModel, globalEnvOpen } = useDux()

  return (
    <Dialog
      open={globalEnvOpen}
      onOpenChange={(o) => {
        if (!o) closeGlobalEnv()
      }}
    >
      {globalEnvOpen && <GlobalEnvForm env={viewModel?.global_env ?? {}} />}
    </Dialog>
  )
}
