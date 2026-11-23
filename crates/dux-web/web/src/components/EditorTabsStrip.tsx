import { X } from "lucide-react"

import { FileTreeIcon } from "@/components/FileTreeIcon"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { TabPill, TabStrip } from "@/components/TabStrip"
import { UnsavedDot } from "@/components/UnsavedDot"
import { fileIconKind } from "@/lib/fileIcons"
import type { EditorTab } from "@/lib/editorTabs"
import {
  editorActivateTab,
  editorCloseTab,
  editorPinTab,
  openEditorCloseTab,
  useDux,
} from "@/lib/store"
import { rootKey, type EditorRoot } from "@/lib/editorRoot"

// The code-editor tab strip: editor tabs are pure client state (see
// lib/editorTabs.ts), not server-sourced. Renders only when the session has at least one tab; with
// zero tabs the body's "Select a file" empty state covers it, so this returns
// null rather than an empty bar.
export function EditorTabsStrip({ root }: { root: EditorRoot }) {
  const { editorTabs } = useDux()
  const state = editorTabs[rootKey(root)]
  const tabs = state?.tabs ?? []
  if (tabs.length === 0) return null

  function requestClose(tab: EditorTab) {
    if (tab.dirty) {
      openEditorCloseTab(root, tab.id)
    } else {
      editorCloseTab(root, tab.id)
    }
  }

  return (
    // The editor fills its overlay or its browser tab, so no pane divider meets
    // the strip's end.
    <TabStrip label="Open files" besidePaneDivider={false}>
      {tabs.map((tab) => (
        <EditorTabPill
          key={tab.id}
          tab={tab}
          active={tab.id === state?.activeId}
          onActivate={() => editorActivateTab(root, tab.id)}
          onPin={() => editorPinTab(root, tab.id)}
          onClose={() => requestClose(tab)}
        />
      ))}
    </TabStrip>
  )
}

function EditorTabPill({
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
      <TabPill
        active={active}
        onSelect={onActivate}
        onDoubleClick={onPin}
        onAuxClick={(e) => {
          if (e.button === 1) onClose()
        }}
        icon={<FileTreeIcon kind={fileIconKind(tab.path)} />}
        label={basename}
        italic={tab.preview}
        // Always shows the close ✕; the dot appears beside it when dirty.
        marker={tab.dirty && <UnsavedDot />}
        trailing={
          <button
            type="button"
            aria-label={`Close ${basename}`}
            onClick={(e) => {
              e.stopPropagation()
              onClose()
            }}
            // max-md:size-8 is a 32px tap area inside the phone pill's 36px height.
            // A tablet keeps the 20px box and an invisible 10px margin takes the tap
            // area to 40px tall and 30px wide, reaching up, down and right only: a
            // margin over the name or dirty dot would turn a select into a close,
            // with no confirm on a clean tab.
            className="relative flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground after:absolute after:-inset-y-2.5 after:-right-2.5 after:left-0 after:hidden hover:text-foreground max-md:size-8 md:pointer-coarse:after:block"
          >
            <X className="size-3.5" />
          </button>
        }
      />
    </SimpleTooltip>
  )
}
