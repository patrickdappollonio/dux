import { Info, Loader2, TriangleAlert } from "lucide-react"

import { lazy, Suspense, useState } from "react"

import { ChunkBoundary } from "@/components/ChunkBoundary"
import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { Button } from "@/components/ui/button"
import { InlineCode } from "@/components/ui/inline-code"
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

// Mounted only once the raw config has loaded, so the editor starts from a
// settled value. In-progress edits survive a failed save, whose parse error is
// shown inline with the modal left open. The save chord inside Monaco saves the
// current draft too.
function ConfigEditorForm({
  text,
  setText,
  error,
  conflict,
}: {
  text: string
  setText: (text: string) => void
  error: string | null
  conflict: string | null
}) {
  return (
    <>
      <div className="h-[60dvh] shrink-0 overflow-hidden rounded-md border border-border max-md:h-auto max-md:min-h-48 max-md:flex-1">
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
    </>
  )
}

// The Monaco config.toml editor. The server validates the TOML before writing,
// and saving persists the file without applying it: the running config is
// unchanged until a reload. A callout in the form says so.
export function ConfigEditorDialog() {
  const { configEditorOpen } = useDux()
  const life = useModalLifetime(configEditorOpen)
  // Keyed by the open, so each one starts from the file as it is read then.
  return life.mounted ? (
    <ConfigEditor key={life.epoch} open={configEditorOpen} onClosed={life.onClosed} />
  ) : null
}

function ConfigEditor({ open, onClosed }: { open: boolean; onClosed: () => void }) {
  const state = useDux()
  // Closing clears the store's copy. Held, a failed read stays a failed read
  // while it fades, never an empty editor with a Save under the pointer.
  const content = useHeldWhileClosed(open, state.configEditorContent)
  const loading = useHeldWhileClosed(open, state.configEditorLoading)
  const error = useHeldWhileClosed(open, state.configEditorError)
  const conflict = useHeldWhileClosed(open, state.configEditorConflict ?? null)
  // The draft lives here rather than in the form, so loading, a failed read and
  // the form are one popup: remounting it once the file arrives would move
  // focus. Each read that lands seeds the draft afresh.
  const [text, setText] = useState(content)
  const [seenLoading, setSeenLoading] = useState(loading)
  if (seenLoading !== loading) {
    setSeenLoading(loading)
    if (!loading) setText(content)
  }
  const loadFailed = !loading && error !== null && !content

  return (
    <Modal
      kind="form"
      open={open}
      onClose={closeConfigEditor}
      onClosed={onClosed}
      size="4xl"
      title={
        <>
          Edit <InlineCode>config.toml</InlineCode>
        </>
      }
      description="Edit the dux configuration. It is validated before saving; invalid TOML is rejected with the reason. Saving does not apply the change; run “Reload config” afterwards."
      submit={
        loadFailed
          ? { label: "Retry", tone: "primary", onSelect: () => openConfigEditor() }
          : {
              label: "Save",
              tone: "primary",
              onSelect: () => saveConfigEditor(text),
              disabledReason: loading ? "The file is still loading." : null,
            }
      }
    >
      {loading ? (
        <p className="py-12 text-center text-sm text-muted-foreground">
          Loading config.toml…
        </p>
      ) : loadFailed ? (
        // Load failed (no content). Never render an editable, Save-enabled
        // editor here: saving its blank content would overwrite the real
        // config.toml. Show the error and a Retry instead.
        <p className="py-8 text-center text-sm text-destructive">{error}</p>
      ) : (
        <ConfigEditorForm text={text} setText={setText} error={error} conflict={conflict} />
      )}
    </Modal>
  )
}
