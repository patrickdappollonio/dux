import { Info, Loader2, TriangleAlert } from "lucide-react"

import { lazy, Suspense, useState } from "react"

import { ChunkBoundary } from "@/components/ChunkBoundary"
import { FormDialogContent } from "@/components/FormDialogContent"
import { Button } from "@/components/ui/button"
import { Dialog } from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  closeConfigEditor,
  dismissConfigEditorConflict,
  openConfigEditor,
  saveConfigEditor,
  useDux,
} from "@/lib/store"

// This dialog mounts closed in `GlobalOverlays`, so a static `CodeEditor` import
// would drag the whole Monaco chunk into the eager index bundle. Lazy-loaded as
// `EditorBody` is, sharing the self-host bootstrap so the chunk loads once.
const CodeEditor = lazy(() => import("@/components/CodeEditor"))

// Mounted only once the raw config has loaded, so the lazy `useState`
// initializer seeds the editor from a settled value. In-progress edits live in
// `text` and survive a failed save, whose parse error is shown inline with the
// modal left open. The save chord inside Monaco saves the current draft too.
function ConfigEditorForm({
  phone,
  initial,
  error,
  conflict,
}: {
  phone: boolean
  initial: string
  error: string | null
  conflict: string | null
}) {
  const [text, setText] = useState(() => initial)

  return (
    <FormDialogContent
      {...shellProps(phone)}
      footer={
        <>
          <Button variant="outline" onClick={closeConfigEditor}>
            Cancel
          </Button>
          <Button onClick={() => saveConfigEditor(text)}>Save</Button>
        </>
      }
    >
      <div className="h-[60vh] shrink-0 overflow-hidden rounded-md border border-border max-md:h-auto max-md:min-h-48 max-md:flex-1">
        {/* ChunkBoundary wraps Suspense (not inside it) so a failed lazy
            import after a redeploy surfaces its reload-needed error instead
            of an eternal spinner (same idiom as EditorBody). */}
        <ChunkBoundary>
          <Suspense
            fallback={
              <div className="flex h-full items-center justify-center text-muted-foreground">
                <Loader2 className="size-5 motion-safe:animate-spin" />
              </div>
            }
          >
            <CodeEditor
              path="config.toml"
              value={text}
              onChange={setText}
              onSave={() => saveConfigEditor(text)}
            />
          </Suspense>
        </ChunkBoundary>
      </div>
      {conflict ? (
        // The file changed on disk since it was opened, and nothing was saved.
        // A choice, not an error: reloading discards these edits, so "Keep
        // editing" has focus and the reload is styled as the destructive one.
        <div
          role="alert"
          className="flex flex-col gap-3 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm"
        >
          <div className="flex items-start gap-2">
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden />
            <span>{conflict}</span>
          </div>
          <div className="flex justify-end gap-3">
            <Button variant="outline" autoFocus onClick={dismissConfigEditorConflict} className="max-md:min-h-10 pointer-coarse:min-h-10">
              Keep editing
            </Button>
            <Button variant="destructive" onClick={() => openConfigEditor()} className="max-md:min-h-10 pointer-coarse:min-h-10">
              Reload the file
            </Button>
          </div>
        </div>
      ) : null}
      {error ? (
        <p className="max-h-24 overflow-y-auto font-mono text-sm break-words whitespace-pre-wrap text-destructive">
          {error}
        </p>
      ) : null}
      <div className="flex items-start gap-2 rounded-md border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
        <Info className="mt-0.5 size-4 shrink-0" aria-hidden />
        <span>
          Saving writes <InlineCode>config.toml</InlineCode> to disk but
          does not apply it. Run <span className="font-medium">Reload config</span>{" "}
          from the app menu&rsquo;s <span className="font-medium">Configuration</span>{" "}
          submenu to apply your changes.
        </span>
      </div>
    </FormDialogContent>
  )
}

function shellProps(phone: boolean) {
  return {
    phone,
    title: (
      <>
        Edit <InlineCode>config.toml</InlineCode>
      </>
    ),
    description:
      "Edit the dux configuration. It is validated before saving; invalid TOML is rejected with the reason. Saving does not apply the change; run “Reload config” afterwards.",
    onClose: closeConfigEditor,
    className: "sm:max-w-4xl",
  }
}

// The Monaco config.toml editor. The server validates the TOML before writing,
// and saving persists the file without applying it: the running config is
// unchanged until a reload. A callout in the form says so.
export function ConfigEditorDialog() {
  const {
    configEditorOpen,
    configEditorContent,
    configEditorLoading,
    configEditorError,
    configEditorConflict,
  } = useDux()
  const phone = useIsMobile()

  return (
    <Dialog
      open={configEditorOpen}
      onOpenChange={(o) => {
        if (!o) closeConfigEditor()
      }}
    >
      {configEditorLoading ? (
        <FormDialogContent {...shellProps(phone)}>
          <p className="py-12 text-center text-sm text-muted-foreground">
            Loading config.toml…
          </p>
        </FormDialogContent>
      ) : configEditorError && !configEditorContent ? (
        // Load failed (no content). Never render an editable, Save-enabled
        // editor here: saving its blank content would overwrite the real
        // config.toml. Show the error and a Retry instead.
        <FormDialogContent
          {...shellProps(phone)}
          footer={
            <>
              <Button variant="outline" onClick={closeConfigEditor}>
                Cancel
              </Button>
              <Button onClick={() => openConfigEditor()}>Retry</Button>
            </>
          }
        >
          <p className="py-8 text-center text-sm text-destructive">{configEditorError}</p>
        </FormDialogContent>
      ) : (
        <ConfigEditorForm
          phone={phone}
          initial={configEditorContent}
          error={configEditorError}
          conflict={configEditorConflict ?? null}
        />
      )}
    </Dialog>
  )
}
