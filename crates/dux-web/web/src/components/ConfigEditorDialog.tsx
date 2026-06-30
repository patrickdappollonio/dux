import { Info } from "lucide-react"
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
import {
  closeConfigEditor,
  openConfigEditor,
  saveConfigEditor,
  useDux,
} from "@/lib/store"

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
      <div className="flex items-start gap-2 rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
        <Info className="mt-0.5 size-4 shrink-0" aria-hidden />
        <span>
          Saving writes <span className="font-mono">config.toml</span> to disk but
          does not apply it. Run <span className="font-medium">Reload config</span>{" "}
          from the command palette (Ctrl-K) to apply your changes.
        </span>
      </div>
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
// TOML before writing; saving PERSISTS the file but does not apply it (the
// running config is unchanged until the user runs "Reload config"). A callout in
// the form states this.
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
            Edit the dux configuration. It is validated before saving; invalid
            TOML is rejected with the reason. Saving does not apply the change —
            run “Reload config” afterwards.
          </DialogDescription>
        </DialogHeader>
        {configEditorLoading ? (
          <p className="py-12 text-center text-sm text-muted-foreground">
            Loading config.toml…
          </p>
        ) : configEditorError && !configEditorContent ? (
          // Load failed (no content). Never render an editable, Save-enabled
          // editor here — saving its blank content would overwrite the real
          // config.toml. Show the error and a Retry instead.
          <div className="flex flex-col gap-4 py-8">
            <p className="text-center text-sm text-destructive">
              {configEditorError}
            </p>
            <DialogFooter>
              <Button variant="outline" onClick={closeConfigEditor}>
                Cancel
              </Button>
              <Button onClick={() => openConfigEditor()}>Retry</Button>
            </DialogFooter>
          </div>
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
