import { useState } from "react"

import CodeEditor from "@/components/CodeEditor"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { closeConfigEditor, saveConfigEditor, useDux } from "@/lib/store"

// The form body is mounted only once the raw config has loaded, so its lazy
// `useState` initializer seeds the editor from a settled value (no
// set-state-in-effect). The user's in-progress edits live in `text` and survive
// a failed save (the server's parse error is shown inline and the modal stays
// open). Ctrl/Cmd+S inside Monaco saves the current draft too.
function ConfigEditorForm({
  initial,
  error,
}: {
  initial: string
  error: string | null
}) {
  const [text, setText] = useState(() => initial)

  return (
    <>
      <div className="h-[60vh] overflow-hidden rounded-md border border-border">
        <CodeEditor
          path="config.toml"
          value={text}
          onChange={setText}
          onSave={() => saveConfigEditor(text)}
        />
      </div>
      {error ? (
        <p className="max-h-24 overflow-y-auto font-mono text-sm whitespace-pre-wrap text-destructive">
          {error}
        </p>
      ) : null}
      <DialogFooter>
        <Button variant="outline" onClick={closeConfigEditor}>
          Cancel
        </Button>
        <Button onClick={() => saveConfigEditor(text)}>Save</Button>
      </DialogFooter>
    </>
  )
}

// The Monaco config.toml editor (Ctrl+K "edit-config"). The server validates the
// TOML before writing and the change is reloaded automatically on save.
export function ConfigEditorDialog() {
  const {
    configEditorOpen,
    configEditorContent,
    configEditorLoading,
    configEditorError,
  } = useDux()

  return (
    <Dialog
      open={configEditorOpen}
      onOpenChange={(o) => {
        if (!o) closeConfigEditor()
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-4xl">
        <DialogHeader>
          <DialogTitle>Edit config.toml</DialogTitle>
          <DialogDescription>
            Edit the dux configuration. It is validated before saving and
            reloaded automatically; invalid TOML is rejected with the reason.
          </DialogDescription>
        </DialogHeader>
        {configEditorLoading ? (
          <p className="py-12 text-center text-sm text-muted-foreground">
            Loading config.toml…
          </p>
        ) : (
          <ConfigEditorForm
            initial={configEditorContent}
            error={configEditorError}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}
