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
import { rootKey, type EditorRoot } from "@/lib/editorRoot"

// The code-editor tab strip (pills), rhymes with `AgentTabsStrip` but is its
// own component: editor tabs are pure client state (see lib/editorTabs.ts),
// not server-sourced. Renders only when the session has at least one tab; with
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
    <div className="flex items-center gap-1 overflow-x-auto border-b bg-muted/30 px-2 py-1">
      {tabs.map((tab) => (
        <TabPill
          key={tab.id}
          tab={tab}
          active={tab.id === state?.activeId}
          onActivate={() => editorActivateTab(root, tab.id)}
          onPin={() => editorPinTab(root, tab.id)}
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
        // max-md:min-h-8.5 makes the phone pill 34px, deliberately under the
        // 40px touch-target floor on the vertical axis only: the strip sits
        // between the header and Monaco, neither a tap target beside a pill,
        // and vertical space is the scarce resource on a phone. max-md:py-0
        // goes with it, or the close button's own hit area adds the padding
        // back and overshoots.
        className={cn(
          "group/etab flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-sm transition-colors max-md:min-h-8.5 max-md:py-0",
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
          // max-md:size-8 keeps a larger-than-visual tap area (32px against
          // the icon's 14px glyph) without adding to the pill's 34px height:
          // the cheap tap forgiveness the reduced pill still affords.
          //
          // A tablet keeps the 20px box and the pill's height; an invisible
          // 10px margin takes the tap area to 40px tall and 30px wide. It
          // reaches up, down and right only: the left edge stays on the box,
          // because a margin over the pill's own name or dirty dot would make
          // a tap meant to select the tab close it, with no confirm on a clean
          // one. The right 10px lies over the pill's own 9px of padding and
          // 1px of the 4px gap to the next pill. Vertically the strip's
          // scroll box clips it to 38px; the neighbours there are the editor
          // header and Monaco, neither a tap target beside the ✕.
          className="relative flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground after:absolute after:-inset-y-2.5 after:-right-2.5 after:left-0 after:hidden hover:text-foreground max-md:size-8 md:pointer-coarse:after:block"
        >
          <X className="size-3.5" />
        </button>
      </div>
    </SimpleTooltip>
  )
}
