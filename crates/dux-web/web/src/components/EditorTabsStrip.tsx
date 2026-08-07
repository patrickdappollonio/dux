import { X } from "lucide-react"

import { FileTreeIcon } from "@/components/FileTreeIcon"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { fileIconKind } from "@/lib/fileIcons"
import type { EditorTab } from "@/lib/editorTabs"
import {
  editorActivateTab,
  editorCloseTab,
  editorPinTab,
  openEditorCloseTab,
  useDux,
} from "@/lib/store"
import { cn } from "@/lib/utils"

// The code-editor tab strip (pills), rhymes with `AgentTabsStrip` but is its
// own component: editor tabs are pure client state (see lib/editorTabs.ts),
// not server-sourced. Renders only when the session has at least one tab; with
// zero tabs the body's "Select a file" empty state covers it, so this returns
// null rather than an empty bar.
export function EditorTabsStrip({ sessionId }: { sessionId: string }) {
  const { editorTabs } = useDux()
  const state = editorTabs[sessionId]
  const tabs = state?.tabs ?? []
  if (tabs.length === 0) return null

  function requestClose(tab: EditorTab) {
    if (tab.dirty) {
      openEditorCloseTab(sessionId, tab.id)
    } else {
      editorCloseTab(sessionId, tab.id)
    }
  }

  return (
    <div className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1">
      {tabs.map((tab) => (
        <TabPill
          key={tab.id}
          tab={tab}
          active={tab.id === state?.activeId}
          onActivate={() => editorActivateTab(sessionId, tab.id)}
          onPin={() => editorPinTab(sessionId, tab.id)}
          onClose={() => requestClose(tab)}
        />
      ))}
    </div>
  )
}

function TabPill({
  tab,
  active,
  onActivate,
  onPin,
  onClose,
}: {
  tab: EditorTab
  active: boolean
  onActivate: () => void
  onPin: () => void
  onClose: () => void
}) {
  const basename = tab.path.split("/").pop() ?? tab.path

  return (
    <SimpleTooltip content={tab.path}>
      <div
        role="tab"
        aria-selected={active}
        tabIndex={0}
        onClick={onActivate}
        onDoubleClick={onPin}
        onAuxClick={(e) => {
          if (e.button === 1) onClose()
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault()
            onActivate()
          }
        }}
        className={cn(
          "group/etab flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors max-md:min-h-10",
          active
            ? "border-border bg-background text-foreground"
            : "border-transparent bg-muted text-muted-foreground hover:text-foreground",
        )}
      >
        <FileTreeIcon kind={fileIconKind(tab.path)} />
        <span
          // `pr-0.5` is inside the truncating (overflow-hidden) box on
          // purpose: an italic final ascender leans past the content edge and
          // gets clipped without it. Unconditional so a preview tab pinning
          // itself never shifts the label.
          className={cn("max-w-40 truncate pr-0.5", tab.preview && "italic")}
        >
          {basename}
        </span>
        {/* Dirty dot to the left of the close ✕ (the plan's simpler accepted
            variant: always show ✕, dot appears alongside it when dirty). */}
        {tab.dirty && (
          <>
            <SimpleTooltip content="Unsaved changes">
              <span className="shrink-0 text-primary" aria-hidden="true">
                ●
              </span>
            </SimpleTooltip>
            <span className="sr-only">unsaved changes</span>
          </>
        )}
        <button
          type="button"
          aria-label={`Close ${basename}`}
          onClick={(e) => {
            e.stopPropagation()
            onClose()
          }}
          className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground max-md:size-10"
        >
          <X className="size-3.5" />
        </button>
      </div>
    </SimpleTooltip>
  )
}
